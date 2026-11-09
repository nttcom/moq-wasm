use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use bytes::Bytes;
use tokio::sync::{mpsc, watch};

use crate::modules::{
    executor::{self, JoinHandle},
    transport::{
        qmux::{
            frame::{ConnectionClose, Frame, ProtocolError, TransportErrorCode},
            message_transport::MessageSource,
            qmux_receive_stream::QmuxReceiveStream,
            qmux_send_stream::QmuxSendStream,
            session_state::{Command, SessionState, TransportEnded},
            stats::Stats,
            stream_id::{StreamKind, stream_index},
            tasks::qmux_send_task::CLOSE_TIMEOUT,
        },
        transport_connection::{BoxedReceiveStream, BoxedSendStream, TransportClose},
    },
};

/// draft-lcurley-qmux-websocket-00 §9 suggests 30 s. Also advertised as
/// `max_idle_timeout`.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct IncomingStreams {
    pub(crate) bidirectional: mpsc::UnboundedSender<(BoxedSendStream, BoxedReceiveStream)>,
    pub(crate) unidirectional: mpsc::UnboundedSender<BoxedReceiveStream>,
    /// Bounded: a datagram that finds the queue full is dropped (§6).
    pub(crate) datagrams: mpsc::Sender<Bytes>,
}

/// Reads records from the message source and hands each frame to the state
/// it belongs to. It never waits for the application (§6): stream data goes
/// into the stream's buffer, bounded by the window this side advertised.
pub(crate) struct QmuxReceiveTask {
    _handle: JoinHandle,
}

impl QmuxReceiveTask {
    pub(crate) fn run<S: MessageSource>(
        source: S,
        session: Arc<SessionState>,
        incoming: IncomingStreams,
        ended: Arc<TransportEnded>,
    ) -> Self {
        let closed = session.closed();
        let receiver = Receiver {
            source,
            session,
            incoming,
            closed,
            parameters_received: false,
            peer_opened: Opened::default(),
            connection_received: 0,
            last_ping_received: None,
        };
        Self {
            _handle: executor::spawn("QMux Receiver", async move {
                let _ended = ended;
                receiver.run().await;
            }),
        }
    }
}

#[derive(Default)]
struct Opened {
    bidirectional: u64,
    unidirectional: u64,
}

impl Opened {
    fn get(&mut self, kind: StreamKind) -> &mut u64 {
        match kind {
            StreamKind::Bidirectional => &mut self.bidirectional,
            StreamKind::Unidirectional => &mut self.unidirectional,
        }
    }
}

struct Receiver<S: MessageSource> {
    source: S,
    session: Arc<SessionState>,
    incoming: IncomingStreams,
    closed: watch::Receiver<Option<TransportClose>>,
    parameters_received: bool,
    /// Streams the peer opened, per kind.
    peer_opened: Opened,
    /// Bytes the peer sent across all streams, against MAX_DATA.
    connection_received: u64,
    last_ping_received: Option<u64>,
}

impl<S: MessageSource> Receiver<S> {
    async fn run(mut self) {
        loop {
            let received = tokio::select! {
                received = executor::timeout(IDLE_TIMEOUT, self.source.receive()) => received,
                _ = self.closed.changed() => return self.drain().await,
            };
            let message = match received {
                Ok(Ok(Some(message))) => message,
                Ok(Ok(None)) => return self.lost("transport ended by the peer"),
                Ok(Err(error)) => return self.lost(&format!("transport receive failed: {error}")),
                Err(()) => {
                    return self.lost(&format!(
                        "idle timeout: nothing received for {} s",
                        IDLE_TIMEOUT.as_secs()
                    ));
                }
            };
            Stats::add(&self.session.stats.received_bytes, message.len() as u64);
            if let Err(error) = self.on_message(message) {
                self.fail(error);
                return self.drain().await;
            }
            if self.session.is_closed() {
                return self.drain().await;
            }
        }
    }

    /// After this side closed, keeps reading until the peer's Close frame or
    /// `CLOSE_TIMEOUT`: a TCP socket closed with unread bytes answers with
    /// RST, which can discard the CONNECTION_CLOSE still in the send buffer.
    async fn drain(mut self) {
        let _ = executor::timeout(CLOSE_TIMEOUT, async {
            while let Ok(Some(_)) = self.source.receive().await {}
        })
        .await;
    }

    fn lost(&self, reason: &str) {
        tracing::debug!(reason, "QMux session lost");
        self.session.set_closed(TransportClose {
            code: None,
            reason: reason.to_string(),
        });
        let _ = self.session.commands.send(Command::Stop);
    }

    /// A violation by the peer: CONNECTION_CLOSE with the transport error code.
    fn fail(&self, error: ProtocolError) {
        tracing::warn!(%error, "QMux protocol violation by the peer");
        self.session.close(
            ConnectionClose::Transport {
                error_code: error.code as u64,
                frame_type: 0,
                reason: error.reason.clone(),
            },
            format!("protocol violation by the peer: {error}"),
        );
    }

    fn on_message(&mut self, message: Bytes) -> Result<(), ProtocolError> {
        if message.len() as u64 > self.session.local.max_record_size {
            return Err(ProtocolError::new(
                TransportErrorCode::FrameEncodingError,
                format!("record of {} bytes exceeds max_record_size", message.len()),
            ));
        }
        for frame in Frame::decode_record(message)? {
            if !self.parameters_received && !matches!(frame, Frame::TransportParameters(_)) {
                return Err(ProtocolError::new(
                    TransportErrorCode::ProtocolViolation,
                    "the first frame is not QX_TRANSPORT_PARAMETERS",
                ));
            }
            self.on_frame(frame)?;
            if self.session.is_closed() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn on_frame(&mut self, frame: Frame) -> Result<(), ProtocolError> {
        match frame {
            Frame::TransportParameters(parameters) => {
                if self.parameters_received {
                    return Err(ProtocolError::new(
                        TransportErrorCode::ProtocolViolation,
                        "QX_TRANSPORT_PARAMETERS received twice",
                    ));
                }
                self.parameters_received = true;
                self.session
                    .peer_max_data
                    .fetch_max(parameters.initial_max_data, Ordering::AcqRel);
                self.session.wake();
                let _ = self
                    .session
                    .commands
                    .send(Command::PeerParameters(parameters.clone()));
                self.session.set_peer_max_streams(
                    StreamKind::Bidirectional,
                    parameters.initial_max_streams_bidi,
                );
                self.session.set_peer_max_streams(
                    StreamKind::Unidirectional,
                    parameters.initial_max_streams_uni,
                );
                self.session.peer_parameters.send_replace(Some(parameters));
            }
            Frame::Stream {
                stream_id,
                offset,
                data,
                fin,
            } => {
                let Some(shared) = self.receive_shared(stream_id)? else {
                    return Err(ProtocolError::new(
                        TransportErrorCode::StreamStateError,
                        format!("STREAM data on stream {stream_id} after it ended"),
                    ));
                };
                let received = shared.push(offset, data, fin)?;
                self.account(stream_id, received)?;
            }
            Frame::ResetStream {
                stream_id,
                error_code,
                final_size,
            } => {
                Stats::add(&self.session.stats.received_reset_stream, 1);
                // RFC 9000 §3.1: a sender may reset a stream after its FIN.
                let Some(shared) = self.receive_shared(stream_id)? else {
                    return Ok(());
                };
                let received = shared.reset(error_code, final_size)?;
                self.account(stream_id, received)?;
            }
            Frame::StopSending {
                stream_id,
                error_code,
            } => {
                Stats::add(&self.session.stats.received_stop_sending, 1);
                if !self.session.role.opened_by_me(stream_id)
                    && StreamKind::of(stream_id) == StreamKind::Unidirectional
                {
                    return Err(ProtocolError::new(
                        TransportErrorCode::StreamStateError,
                        "STOP_SENDING for a stream only the peer sends on",
                    ));
                }
                if let Some(quota) = self.send_quota(stream_id) {
                    quota.stopped(error_code);
                    self.session.wake();
                }
            }
            Frame::MaxData { maximum } => {
                self.session
                    .peer_max_data
                    .fetch_max(maximum, Ordering::AcqRel);
                self.session.wake();
            }
            Frame::MaxStreamData { stream_id, maximum } => {
                Stats::add(&self.session.stats.received_max_stream_data, 1);
                if let Some(quota) = self.send_quota(stream_id) {
                    quota.raise_peer_limit(maximum);
                    self.session.wake();
                }
            }
            Frame::MaxStreams {
                bidirectional,
                maximum,
            } => {
                let kind = if bidirectional {
                    StreamKind::Bidirectional
                } else {
                    StreamKind::Unidirectional
                };
                self.session.set_peer_max_streams(kind, maximum);
            }
            Frame::DataBlocked { limit } => {
                Stats::add(&self.session.stats.received_data_blocked, 1);
                tracing::debug!(limit, "peer is blocked by MAX_DATA");
            }
            Frame::StreamDataBlocked { stream_id, limit } => {
                Stats::add(&self.session.stats.received_stream_data_blocked, 1);
                tracing::debug!(stream_id, limit, "peer is blocked by MAX_STREAM_DATA");
            }
            Frame::StreamsBlocked {
                bidirectional,
                limit,
            } => {
                tracing::debug!(bidirectional, limit, "peer is blocked by MAX_STREAMS");
            }
            Frame::Datagram { data } => {
                if self.session.local.max_datagram_frame_size.is_none() {
                    return Err(ProtocolError::new(
                        TransportErrorCode::ProtocolViolation,
                        "DATAGRAM frame without the datagram extension",
                    ));
                }
                if self.incoming.datagrams.try_send(data).is_err() {
                    tracing::debug!("datagram dropped: the application is not reading them");
                }
            }
            Frame::Ping { sequence } => {
                if self.last_ping_received.is_some_and(|last| sequence <= last) {
                    return Err(ProtocolError::new(
                        TransportErrorCode::ProtocolViolation,
                        "QX_PING sequence number did not increase",
                    ));
                }
                self.last_ping_received = Some(sequence);
                // §4.3: one response for several pings, with the largest number.
                *self
                    .session
                    .pending_ping_response
                    .lock()
                    .expect("ping lock") = Some(sequence);
                self.session.wake();
            }
            Frame::PingResponse { sequence } => {
                let in_flight = *self.session.ping_in_flight.lock().expect("ping lock");
                match in_flight {
                    Some((sent, sent_at)) if sequence == sent => {
                        self.session
                            .stats
                            .set_rtt(executor::now().saturating_sub(sent_at));
                    }
                    Some((sent, _)) if sequence > sent => {
                        return Err(ProtocolError::new(
                            TransportErrorCode::ProtocolViolation,
                            "QX_PING response to a ping that was never sent",
                        ));
                    }
                    _ => {}
                }
            }
            Frame::ConnectionClose(close) => {
                let transport_close = match close {
                    ConnectionClose::Application { error_code, reason } => TransportClose {
                        code: u32::try_from(error_code).ok(),
                        reason,
                    },
                    ConnectionClose::Transport {
                        error_code, reason, ..
                    } => TransportClose {
                        code: None,
                        reason: format!(
                            "peer closed with transport error {error_code:#x}: {reason}"
                        ),
                    },
                };
                tracing::debug!(?transport_close, "QMux session closed by the peer");
                self.session.set_closed(transport_close);
                let _ = self.session.commands.send(Command::Stop);
            }
        }
        Ok(())
    }

    /// Flow control after a STREAM frame or RESET_STREAM: what the peer sent
    /// counts against MAX_DATA, what nobody will read is credited back.
    fn account(
        &mut self,
        stream_id: u64,
        received: super::super::qmux_receive_stream::Received,
    ) -> Result<(), ProtocolError> {
        self.connection_received += received.counted;
        if self.connection_received > self.session.connection_window_end() {
            return Err(ProtocolError::new(
                TransportErrorCode::FlowControlError,
                "peer sent more than MAX_DATA",
            ));
        }
        if received.discarded > 0 {
            self.session.consumed(received.discarded);
        }
        if received.ended {
            self.stream_ended(stream_id);
        }
        if received.freed && !self.session.role.opened_by_me(stream_id) {
            self.session.release_peer_stream(StreamKind::of(stream_id));
        }
        Ok(())
    }

    /// The receiving state of `stream_id`, opening the stream when the peer
    /// has not sent on it before (RFC 9000 §3.2: a stream id also opens the
    /// lower-numbered streams of its kind). `None` for a stream whose data
    /// already ended, or one this side opened and never receives on.
    fn receive_shared(
        &mut self,
        stream_id: u64,
    ) -> Result<Option<Arc<super::super::qmux_receive_stream::ReceiveShared>>, ProtocolError> {
        if let Some(shared) = self
            .session
            .receive_streams
            .lock()
            .expect("receive streams lock")
            .get(&stream_id)
        {
            return Ok(Some(shared.clone()));
        }
        let kind = StreamKind::of(stream_id);
        if self.session.role.opened_by_me(stream_id) {
            return Ok(None);
        }
        let index = stream_index(stream_id);
        let opened = *self.peer_opened.get(kind);
        if index < opened {
            return Ok(None);
        }
        if index >= self.advertised_max_streams(kind) {
            return Err(ProtocolError::new(
                TransportErrorCode::StreamLimitError,
                format!("peer opened stream {stream_id} beyond MAX_STREAMS"),
            ));
        }
        for implicit in opened..=index {
            self.open_peer_stream(self.session.role.peer_stream_id(kind, implicit));
        }
        *self.peer_opened.get(kind) = index + 1;
        let shared = self
            .session
            .receive_streams
            .lock()
            .expect("receive streams lock")
            .get(&stream_id)
            .cloned()
            .expect("the stream was just opened");
        Ok(Some(shared))
    }

    fn open_peer_stream(&self, stream_id: u64) {
        let local = &self.session.local;
        match StreamKind::of(stream_id) {
            StreamKind::Bidirectional => {
                let shared = self
                    .session
                    .register_receive_stream(stream_id, local.initial_max_stream_data_bidi_remote);
                let quota = self.session.register_send_stream(stream_id);
                let _ = self.incoming.bidirectional.send((
                    Box::new(QmuxSendStream::new(stream_id, self.session.clone(), quota)),
                    Box::new(QmuxReceiveStream::new(
                        stream_id,
                        self.session.clone(),
                        shared,
                    )),
                ));
            }
            StreamKind::Unidirectional => {
                let shared = self
                    .session
                    .register_receive_stream(stream_id, local.initial_max_stream_data_uni);
                let _ = self
                    .incoming
                    .unidirectional
                    .send(Box::new(QmuxReceiveStream::new(
                        stream_id,
                        self.session.clone(),
                        shared,
                    )));
            }
        }
    }

    fn send_quota(&self, stream_id: u64) -> Option<Arc<super::super::qmux_send_stream::SendQuota>> {
        self.session
            .send_quotas
            .lock()
            .expect("send quotas lock")
            .get(&stream_id)
            .cloned()
    }

    /// The streams the peer may have opened: the initial limit plus the ones
    /// the application released (`QmuxReceiveStream::drop`), as quinn frees
    /// a slot when the application is done with the stream, not when the
    /// peer's data is.
    fn advertised_max_streams(&self, kind: StreamKind) -> u64 {
        let initial = match kind {
            StreamKind::Bidirectional => self.session.local.initial_max_streams_bidi,
            StreamKind::Unidirectional => self.session.local.initial_max_streams_uni,
        };
        initial + self.session.released_peer_streams(kind)
    }

    /// The peer's data on `stream_id` is complete.
    fn stream_ended(&mut self, stream_id: u64) {
        self.session
            .receive_streams
            .lock()
            .expect("receive streams lock")
            .remove(&stream_id);
    }
}
