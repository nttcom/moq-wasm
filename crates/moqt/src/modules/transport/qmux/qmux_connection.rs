use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use tokio::sync::mpsc;

use super::{
    frame::{ConnectionClose, Frame, datagram_frame_overhead},
    message_transport::{MessageSink, MessageSource},
    qmux_receive_stream::QmuxReceiveStream,
    qmux_send_stream::QmuxSendStream,
    session_state::{Command, SessionState, TransportEnded},
    stream_id::{Role, StreamKind},
    tasks::{
        qmux_receive_task::{IDLE_TIMEOUT, IncomingStreams, QmuxReceiveTask},
        qmux_send_task::QmuxSendTask,
    },
    transport_parameters::{DEFAULT_MAX_RECORD_SIZE, TransportParameters},
};
use crate::modules::transport::{
    application_protocol::NegotiatedProtocol,
    transport_addresses::TransportAddresses,
    transport_connection::{
        BoxedReceiveStream, BoxedSendStream, TransportClose, TransportConnection,
    },
    transport_stats::TransportStats,
};

/// quinn's `stream_receive_window` in `server_transport_config.rs`.
const STREAM_RECEIVE_WINDOW: u64 = 1 << 20;
/// Bounds what every stream together may buffer; quinn leaves this unlimited.
const CONNECTION_RECEIVE_WINDOW: u64 = 16 << 20;
/// quinn's `max_concurrent_uni_streams` in `server_transport_config.rs`.
const MAX_UNIDIRECTIONAL_STREAMS: u64 = 1000;
/// quinn's default; MoQT opens one bidirectional stream, the control channel.
const MAX_BIDIRECTIONAL_STREAMS: u64 = 100;
/// Datagrams waiting for `receive_datagram`; later ones are dropped (§6).
const DATAGRAM_QUEUE_LENGTH: usize = 64;

fn local_parameters() -> TransportParameters {
    TransportParameters {
        max_idle_timeout: Some(IDLE_TIMEOUT),
        initial_max_data: CONNECTION_RECEIVE_WINDOW,
        initial_max_stream_data_bidi_local: STREAM_RECEIVE_WINDOW,
        initial_max_stream_data_bidi_remote: STREAM_RECEIVE_WINDOW,
        initial_max_stream_data_uni: STREAM_RECEIVE_WINDOW,
        initial_max_streams_bidi: MAX_BIDIRECTIONAL_STREAMS,
        initial_max_streams_uni: MAX_UNIDIRECTIONAL_STREAMS,
        max_datagram_frame_size: Some(DEFAULT_MAX_RECORD_SIZE),
        max_record_size: DEFAULT_MAX_RECORD_SIZE,
    }
}

pub(crate) struct QmuxConnection {
    session: Arc<SessionState>,
    incoming_bidirectional:
        tokio::sync::Mutex<mpsc::UnboundedReceiver<(BoxedSendStream, BoxedReceiveStream)>>,
    incoming_unidirectional: tokio::sync::Mutex<mpsc::UnboundedReceiver<BoxedReceiveStream>>,
    datagrams: tokio::sync::Mutex<mpsc::Receiver<Bytes>>,
    addresses: TransportAddresses,
    /// The application ALPN the WebSocket subprotocol named.
    alpn: Option<String>,
    _send_task: QmuxSendTask,
    _receive_task: QmuxReceiveTask,
}

impl QmuxConnection {
    /// Sends this side's transport parameters and resolves once the peer's
    /// arrived; fails if the transport ends first. The caller bounds the wait.
    pub(crate) async fn establish<K: MessageSink, S: MessageSource>(
        sink: K,
        source: S,
        role: Role,
        alpn: Option<String>,
        addresses: TransportAddresses,
    ) -> anyhow::Result<Self> {
        let local = local_parameters();
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let session = Arc::new(SessionState::new(role, local.clone(), command_sender));
        let _ = session
            .commands
            .send(Command::Frame(Frame::TransportParameters(local)));
        let (bidirectional_sender, bidirectional_receiver) = mpsc::unbounded_channel();
        let (unidirectional_sender, unidirectional_receiver) = mpsc::unbounded_channel();
        let (datagram_sender, datagram_receiver) = mpsc::channel(DATAGRAM_QUEUE_LENGTH);
        let ended = Arc::new(TransportEnded(session.clone()));
        let send_task = QmuxSendTask::run(sink, session.clone(), command_receiver, ended.clone());
        let receive_task = QmuxReceiveTask::run(
            source,
            session.clone(),
            IncomingStreams {
                bidirectional: bidirectional_sender,
                unidirectional: unidirectional_sender,
                datagrams: datagram_sender,
            },
            ended,
        );
        let connection = Self {
            session: session.clone(),
            incoming_bidirectional: tokio::sync::Mutex::new(bidirectional_receiver),
            incoming_unidirectional: tokio::sync::Mutex::new(unidirectional_receiver),
            datagrams: tokio::sync::Mutex::new(datagram_receiver),
            addresses,
            alpn,
            _send_task: send_task,
            _receive_task: receive_task,
        };
        let mut peer_parameters = session.peer_parameters.subscribe();
        let mut closed = session.closed();
        tokio::select! {
            _ = peer_parameters.wait_for(Option::is_some) => Ok(connection),
            close = closed.wait_for(Option::is_some) => {
                let reason = close
                    .ok()
                    .and_then(|close| close.clone())
                    .map(|close| close.reason)
                    .unwrap_or_default();
                anyhow::bail!("QMux session ended before the peer's transport parameters: {reason}")
            }
        }
    }

    /// Becomes `true` once both tasks have returned (`TransportEnded`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn transport_ended(&self) -> tokio::sync::watch::Receiver<bool> {
        self.session.transport_ended()
    }

    async fn open(&self, kind: StreamKind) -> anyhow::Result<(u64, BoxedSendStream)> {
        let stream_id = self.session.allocate_stream_id(kind).await?;
        let quota = self.session.register_send_stream(stream_id);
        let send_stream = QmuxSendStream::new(stream_id, self.session.clone(), quota);
        Ok((stream_id, Box::new(send_stream)))
    }
}

impl std::fmt::Debug for QmuxConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QmuxConnection")
            .field("role", &self.session.role)
            .field("addresses", &self.addresses)
            .finish()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl TransportConnection for QmuxConnection {
    async fn closed(&self) -> TransportClose {
        let mut closed = self.session.closed();
        let close = closed
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|close| close.clone())
            .unwrap_or_else(|| TransportClose {
                code: None,
                reason: "QMux session dropped".into(),
            });
        tracing::info!(?close, "WebSocket connection closed");
        close
    }

    fn close(&self, code: u32, reason: &str) {
        self.session.close(
            ConnectionClose::Application {
                error_code: u64::from(code),
                reason: reason.to_string(),
            },
            format!("closed locally: {reason}"),
        );
        tracing::info!(code, reason, "WebSocket connection close requested");
    }

    async fn open_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)> {
        let (stream_id, send_stream) = self.open(StreamKind::Bidirectional).await?;
        let shared = self.session.register_receive_stream(
            stream_id,
            self.session.local.initial_max_stream_data_bidi_local,
        );
        let receive_stream = QmuxReceiveStream::new(stream_id, self.session.clone(), shared);
        Ok((send_stream, Box::new(receive_stream)))
    }

    async fn accept_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)> {
        self.incoming_bidirectional
            .lock()
            .await
            .recv()
            .await
            .context("QMux session closed")
    }

    async fn open_uni(&self) -> anyhow::Result<BoxedSendStream> {
        let (_, send_stream) = self.open(StreamKind::Unidirectional).await?;
        Ok(send_stream)
    }

    async fn accept_uni(&self) -> anyhow::Result<BoxedReceiveStream> {
        self.incoming_unidirectional
            .lock()
            .await
            .recv()
            .await
            .context("QMux session closed")
    }

    fn send_datagram(&self, bytes: BytesMut) -> anyhow::Result<()> {
        let peer = self.session.peer_parameters.borrow().clone();
        let peer = peer.context("the peer's transport parameters are unknown")?;
        let max_frame_size = peer
            .max_datagram_frame_size
            .context("the peer does not accept datagrams")?;
        let frame_size = (bytes.len() + datagram_frame_overhead(bytes.len())) as u64;
        anyhow::ensure!(
            frame_size <= max_frame_size.min(peer.max_record_size),
            "datagram of {} bytes exceeds what the peer accepts",
            bytes.len()
        );
        self.session
            .commands
            .send(Command::Datagram(bytes.freeze()))
            .map_err(|_| anyhow::anyhow!("QMux session closed"))
    }

    async fn receive_datagram(&self) -> anyhow::Result<BytesMut> {
        let datagram = self
            .datagrams
            .lock()
            .await
            .recv()
            .await
            .context("QMux session closed")?;
        Ok(BytesMut::from(&datagram[..]))
    }

    fn stats(&self) -> TransportStats {
        self.session.stats.snapshot()
    }

    fn addresses(&self) -> TransportAddresses {
        self.addresses
    }

    fn negotiated_protocol(&self) -> NegotiatedProtocol {
        NegotiatedProtocol::WebSocket {
            alpn: self.alpn.clone(),
        }
    }
}

/// Like quinn, a dropped connection closes with application code 0 so the
/// peer learns of it at once instead of through the idle timeout.
impl Drop for QmuxConnection {
    fn drop(&mut self) {
        if !self.session.is_closed() {
            self.session.close(
                ConnectionClose::Application {
                    error_code: 0,
                    reason: String::new(),
                },
                "dropped".to_string(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::{Bytes, BytesMut};

    use super::*;
    use crate::modules::transport::{
        qmux::{
            frame::{Frame, TransportErrorCode},
            in_memory::{InMemorySource, client_with_raw_peer, connected, read_to_end},
            message_transport::{MessageSink, MessageSource},
            transport_parameters::TransportParameters,
        },
        read_error::ReadError,
        transport_send_stream::TransportSendError,
    };

    const TIMEOUT: Duration = Duration::from_secs(5);

    async fn within<F: Future>(future: F) -> F::Output {
        tokio::time::timeout(TIMEOUT, future)
            .await
            .expect("the exchange timed out")
    }

    fn payload(length: usize) -> BytesMut {
        (0..length).map(|i| (i % 251) as u8).collect()
    }

    fn record(frame: Frame) -> Bytes {
        let mut record = BytesMut::new();
        frame.encode(&mut record);
        record.freeze()
    }

    /// The next MAX_STREAMS or CONNECTION_CLOSE the raw peer receives.
    async fn next_limit_frame(raw_source: &mut InMemorySource) -> Frame {
        loop {
            let record = raw_source.receive().await.unwrap().unwrap();
            let found = Frame::decode_record(record)
                .unwrap()
                .into_iter()
                .find(|frame| {
                    matches!(frame, Frame::MaxStreams { .. } | Frame::ConnectionClose(_))
                });
            if let Some(frame) = found {
                return frame;
            }
        }
    }

    #[tokio::test]
    async fn a_unidirectional_stream_delivers_its_data_in_order_and_then_ends() {
        // Arrange
        let (client, server) = connected().await;
        let mut send_stream = client.open_uni().await.unwrap();

        // Act
        send_stream.send(&payload(100)).await.unwrap();
        send_stream.send(&payload(50_000)).await.unwrap();
        send_stream.close().unwrap();
        let mut receive_stream = within(server.accept_uni()).await.unwrap();
        let received = within(read_to_end(&mut receive_stream)).await.unwrap();

        // Assert
        let mut expected = payload(100);
        expected.extend_from_slice(&payload(50_000));
        assert_eq!(received, expected.to_vec());
    }

    #[tokio::test]
    async fn a_bidirectional_stream_carries_data_both_ways() {
        // Arrange
        let (client, server) = connected().await;
        let (mut client_send, mut client_receive) = client.open_bi().await.unwrap();

        // Act
        client_send
            .send(&BytesMut::from(&b"ping"[..]))
            .await
            .unwrap();
        client_send.close().unwrap();
        let (mut server_send, mut server_receive) = within(server.accept_bi()).await.unwrap();
        let request = within(read_to_end(&mut server_receive)).await.unwrap();
        server_send
            .send(&BytesMut::from(&b"pong"[..]))
            .await
            .unwrap();
        server_send.close().unwrap();
        let response = within(read_to_end(&mut client_receive)).await.unwrap();

        // Assert
        assert_eq!(request, b"ping");
        assert_eq!(response, b"pong");
    }

    #[tokio::test]
    async fn data_beyond_the_window_flows_once_the_reader_consumes_it() {
        // Arrange: three times the stream window
        let (client, server) = connected().await;
        let data = payload(3 << 20);
        let mut send_stream = client.open_uni().await.unwrap();
        let sender = {
            let data = data.clone();
            tokio::spawn(async move {
                send_stream.send(&data).await.unwrap();
                send_stream.close().unwrap();
            })
        };

        // Act
        let mut receive_stream = within(server.accept_uni()).await.unwrap();
        let received = within(read_to_end(&mut receive_stream)).await.unwrap();
        within(sender).await.unwrap();

        // Assert
        assert_eq!(received.len(), data.len());
        assert_eq!(received, data.to_vec());
    }

    #[tokio::test]
    async fn a_sender_waits_while_the_reader_does_not_read() {
        // Arrange: more than the stream window plus the send queue
        let (client, server) = connected().await;
        let mut send_stream = client.open_uni().await.unwrap();
        let mut receive_stream = {
            send_stream.send(&payload(1)).await.unwrap();
            within(server.accept_uni()).await.unwrap()
        };
        let sender = tokio::spawn(async move {
            send_stream.send(&payload(2 << 20)).await.unwrap();
            send_stream.close().unwrap();
        });

        // Act: the send cannot finish while nothing is read
        tokio::time::sleep(Duration::from_millis(300)).await;
        let finished_early = sender.is_finished();
        let received = within(read_to_end(&mut receive_stream)).await.unwrap();

        // Assert
        assert!(!finished_early);
        assert_eq!(received.len(), (2 << 20) + 1);
    }

    #[tokio::test]
    async fn a_reset_reaches_the_reader() {
        // Arrange
        let (client, server) = connected().await;
        let mut send_stream = client.open_uni().await.unwrap();
        send_stream.send(&payload(10)).await.unwrap();
        let mut receive_stream = within(server.accept_uni()).await.unwrap();

        // Act
        send_stream.reset(7).await.unwrap();
        let result = within(read_to_end(&mut receive_stream)).await;

        // Assert
        assert_eq!(result, Err(ReadError::Reset));
    }

    #[tokio::test]
    async fn a_dropped_reader_stops_the_sender() {
        // Arrange
        let (client, server) = connected().await;
        let mut send_stream = client.open_uni().await.unwrap();
        send_stream.send(&payload(10)).await.unwrap();
        let receive_stream = within(server.accept_uni()).await.unwrap();

        // Act
        drop(receive_stream);
        let result = within(async {
            loop {
                if let Err(error) = send_stream.send(&payload(16_000)).await {
                    return error;
                }
            }
        })
        .await;

        // Assert
        assert!(matches!(result, TransportSendError::Stopped { code: 0 }));
    }

    #[tokio::test]
    async fn close_carries_the_application_code_to_the_peer() {
        // Arrange
        let (client, server) = connected().await;

        // Act
        client.close(0x10, "bye");
        let server_close = within(server.closed()).await;
        let client_close = within(client.closed()).await;

        // Assert
        assert_eq!(server_close.code, Some(0x10));
        assert_eq!(server_close.reason, "bye");
        assert_eq!(client_close.code, None);
        assert!(within(server.accept_uni()).await.is_err());
    }

    #[tokio::test]
    async fn datagrams_round_trip() {
        // Arrange
        let (client, server) = connected().await;

        // Act
        client.send_datagram(payload(1000)).unwrap();
        let received = within(server.receive_datagram()).await.unwrap();

        // Assert
        assert_eq!(received, payload(1000));
        assert!(client.send_datagram(payload(20_000)).is_err());
    }

    #[tokio::test]
    async fn a_first_frame_other_than_transport_parameters_is_a_protocol_violation() {
        // Arrange
        let ((client_sink, client_source), (mut raw_sink, mut raw_source)) =
            crate::modules::transport::qmux::in_memory::link();
        let mut record = BytesMut::new();
        Frame::MaxData { maximum: 1 }.encode(&mut record);
        raw_sink.send(record.freeze()).await.unwrap();

        // Act
        let result = within(QmuxConnection::establish(
            client_sink,
            client_source,
            Role::Client,
            None,
            TransportAddresses::default(),
        ))
        .await;
        let _parameters = raw_source.receive().await.unwrap().unwrap();
        let close = raw_source.receive().await.unwrap().unwrap();

        // Assert
        assert!(result.is_err());
        assert!(matches!(
            Frame::decode_record(close).unwrap().as_slice(),
            [Frame::ConnectionClose(ConnectionClose::Transport { error_code, .. })]
                if *error_code == TransportErrorCode::ProtocolViolation as u64
        ));
    }

    #[tokio::test]
    async fn data_beyond_the_advertised_window_is_a_flow_control_error() {
        // Arrange: a raw peer that ignores the window this side advertised
        let (client, mut raw_sink, mut raw_source) =
            client_with_raw_peer(TransportParameters::default()).await;
        let chunk = Bytes::from(vec![0u8; 16_000]);

        // Act: stream 3 (the peer's first unidirectional stream) runs past the window
        let mut offset = 0;
        while offset <= STREAM_RECEIVE_WINDOW {
            let mut record = BytesMut::new();
            Frame::Stream {
                stream_id: 3,
                offset,
                data: chunk.clone(),
                fin: false,
            }
            .encode(&mut record);
            raw_sink.send(record.freeze()).await.unwrap();
            offset += chunk.len() as u64;
        }
        let close = within(async {
            loop {
                let record = raw_source.receive().await.unwrap().unwrap();
                if let Some(Frame::ConnectionClose(close)) = Frame::decode_record(record)
                    .unwrap()
                    .into_iter()
                    .find(|frame| matches!(frame, Frame::ConnectionClose(_)))
                {
                    return close;
                }
            }
        })
        .await;

        // Assert
        assert!(matches!(
            close,
            ConnectionClose::Transport { error_code, .. }
                if error_code == TransportErrorCode::FlowControlError as u64
        ));
        assert!(within(client.closed()).await.code.is_none());
    }

    #[tokio::test]
    async fn a_reset_after_the_fin_is_ignored_but_data_after_it_is_not() {
        // Arrange: stream 3 delivered and read to its end
        let (client, mut raw_sink, mut raw_source) =
            client_with_raw_peer(TransportParameters::default()).await;
        raw_sink
            .send(record(Frame::Stream {
                stream_id: 3,
                offset: 0,
                data: Bytes::from_static(b"x"),
                fin: true,
            }))
            .await
            .unwrap();
        let mut ended = within(client.accept_uni()).await.unwrap();
        assert_eq!(within(read_to_end(&mut ended)).await.unwrap(), b"x");

        // Act: RFC 9000 §3.1 lets the peer reset after its FIN; data after it is a violation
        raw_sink
            .send(record(Frame::ResetStream {
                stream_id: 3,
                error_code: 0,
                final_size: 1,
            }))
            .await
            .unwrap();
        raw_sink
            .send(record(Frame::Stream {
                stream_id: 7,
                offset: 0,
                data: Bytes::from_static(b"y"),
                fin: true,
            }))
            .await
            .unwrap();
        let still_open = within(client.accept_uni()).await;
        raw_sink
            .send(record(Frame::Stream {
                stream_id: 3,
                offset: 1,
                data: Bytes::from_static(b"z"),
                fin: false,
            }))
            .await
            .unwrap();
        let close = within(next_limit_frame(&mut raw_source)).await;

        // Assert
        assert!(still_open.is_ok());
        assert!(matches!(
            close,
            Frame::ConnectionClose(ConnectionClose::Transport { error_code, .. })
                if error_code == TransportErrorCode::StreamStateError as u64
        ));
    }

    #[tokio::test]
    async fn a_stream_slot_returns_only_once_the_dropped_stream_has_ended() {
        // Arrange: the peer opens every allowed stream; the application drops each unread
        let (client, mut raw_sink, mut raw_source) =
            client_with_raw_peer(TransportParameters::default()).await;
        let stream_id = |index: u64| Role::Server.stream_id(StreamKind::Unidirectional, index);
        for index in 0..MAX_UNIDIRECTIONAL_STREAMS {
            raw_sink
                .send(record(Frame::Stream {
                    stream_id: stream_id(index),
                    offset: 0,
                    data: Bytes::from_static(b"x"),
                    fin: false,
                }))
                .await
                .unwrap();
            drop(within(client.accept_uni()).await.unwrap());
        }

        // Act: ending one dropped stream frees a slot; one more than that is too many
        raw_sink
            .send(record(Frame::ResetStream {
                stream_id: stream_id(0),
                error_code: 0,
                final_size: 1,
            }))
            .await
            .unwrap();
        let granted = within(next_limit_frame(&mut raw_source)).await;
        raw_sink
            .send(record(Frame::Stream {
                stream_id: stream_id(MAX_UNIDIRECTIONAL_STREAMS + 1),
                offset: 0,
                data: Bytes::from_static(b"x"),
                fin: false,
            }))
            .await
            .unwrap();
        let close = within(next_limit_frame(&mut raw_source)).await;

        // Assert
        assert_eq!(
            granted,
            Frame::MaxStreams {
                bidirectional: false,
                maximum: MAX_UNIDIRECTIONAL_STREAMS + 1,
            }
        );
        assert!(matches!(
            close,
            Frame::ConnectionClose(ConnectionClose::Transport { error_code, .. })
                if error_code == TransportErrorCode::StreamLimitError as u64
        ));
    }

    #[tokio::test]
    async fn the_higher_priority_stream_is_sent_first() {
        // Arrange
        let (client, _raw_sink, mut raw_source) = client_with_raw_peer(TransportParameters {
            initial_max_data: 1 << 20,
            initial_max_stream_data_uni: 1 << 20,
            initial_max_streams_uni: 10,
            ..TransportParameters::default()
        })
        .await;
        let mut low = client.open_uni().await.unwrap();
        let mut high = client.open_uni().await.unwrap();
        low.set_priority(-1).unwrap();
        high.set_priority(1).unwrap();

        // Act: both queue before the send task runs (current-thread runtime)
        low.send(&payload(20_000)).await.unwrap();
        high.send(&payload(20_000)).await.unwrap();
        let first = raw_source.receive().await.unwrap().unwrap();
        let frames = Frame::decode_record(first).unwrap();

        // Assert
        assert!(matches!(
            frames.as_slice(),
            [Frame::Stream { stream_id: 6, .. }]
        ));
    }
}
