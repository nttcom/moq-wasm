use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use bytes::{Bytes, BytesMut};
use tokio::sync::{mpsc, watch};

use crate::modules::{
    executor::{self, JoinHandle},
    transport::{
        qmux::{
            frame::{ConnectionClose, Frame, datagram_frame_overhead, stream_frame_overhead},
            message_transport::MessageSink,
            qmux_send_stream::SendQuota,
            session_state::{Command, SessionState, TransportEnded},
            stats::Stats,
            stream_id::StreamKind,
            tasks::qmux_receive_task::IDLE_TIMEOUT,
            transport_parameters::{DEFAULT_MAX_RECORD_SIZE, TransportParameters},
        },
        transport_connection::TransportClose,
    },
};

/// draft-lcurley-qmux-websocket-00 §9 suggests 5 s; QX_PING (§4.3) is used
/// instead of WebSocket Ping, which a browser cannot send.
const PING_INTERVAL: Duration = Duration::from_secs(5);
/// How long the final CONNECTION_CLOSE and WebSocket Close may take.
pub(crate) const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// quinn's `datagram_send_buffer_size`: beyond it the oldest datagram is
/// dropped (draft §9.1, RFC 9221 §5), so a slow peer costs a bounded amount.
const DATAGRAM_SEND_BUFFER: usize = 1 << 20;

/// The actor that owns the message sink: it turns commands into QMux records,
/// control frames first, then datagrams, then stream data in priority order,
/// within the peer's flow control credit and record size. What the peer
/// grants or asks (credit, STOP_SENDING, QX_PING, a freed stream slot) is
/// read from the shared state on every pass, so the peer cannot queue work.
pub(crate) struct QmuxSendTask {
    _handle: JoinHandle,
}

impl QmuxSendTask {
    pub(crate) fn run<K: MessageSink>(
        sink: K,
        session: Arc<SessionState>,
        commands: mpsc::UnboundedReceiver<Command>,
        ended: Arc<TransportEnded>,
    ) -> Self {
        let closed = session.closed();
        let sender = Sender {
            sink,
            session,
            commands,
            closed,
            peer: None,
            peer_max_data: 0,
            sent_total: 0,
            streams: HashMap::new(),
            control: VecDeque::new(),
            datagrams: VecDeque::new(),
            datagram_bytes: 0,
            next_ping: 1,
            last_ping_at: executor::now(),
            served: 0,
            data_blocked_reported: false,
            granted_bidirectional: 0,
            granted_unidirectional: 0,
        };
        Self {
            _handle: executor::spawn("QMux Sender", async move {
                let _ended = ended;
                sender.run().await;
            }),
        }
    }
}

struct Sender<K: MessageSink> {
    sink: K,
    session: Arc<SessionState>,
    commands: mpsc::UnboundedReceiver<Command>,
    closed: watch::Receiver<Option<TransportClose>>,
    peer: Option<TransportParameters>,
    peer_max_data: u64,
    sent_total: u64,
    streams: HashMap<u64, SendStreamState>,
    control: VecDeque<Frame>,
    datagrams: VecDeque<Bytes>,
    datagram_bytes: usize,
    next_ping: u64,
    last_ping_at: Duration,
    /// A counter that orders streams of equal priority round-robin.
    served: u64,
    data_blocked_reported: bool,
    /// Peer-opened streams the application released that MAX_STREAMS has
    /// already given back, per kind.
    granted_bidirectional: u64,
    granted_unidirectional: u64,
}

struct SendStreamState {
    queue: VecDeque<Bytes>,
    queued_bytes: usize,
    /// Bytes sent so far: the next STREAM frame's offset.
    offset: u64,
    max_stream_data: u64,
    fin_pending: bool,
    priority: i32,
    last_served: u64,
    quota: Arc<SendQuota>,
    stream_data_blocked_reported: bool,
}

impl SendStreamState {
    fn sendable(&self, sent_total: u64, peer_max_data: u64) -> bool {
        if self.queue.is_empty() {
            return self.fin_pending;
        }
        self.offset < self.max_stream_data && sent_total < peer_max_data
    }
}

enum Flow {
    Continue,
    Stop,
}

impl<K: MessageSink> Sender<K> {
    async fn run(mut self) {
        loop {
            if !self.has_work() {
                let since_ping = executor::now().saturating_sub(self.last_ping_at);
                // biased: a Close command and the closed watch it sets arrive
                // together, and only the command carries the CONNECTION_CLOSE frame.
                tokio::select! {
                    biased;
                    command = self.commands.recv() => {
                        let Some(command) = command else { return };
                        if let Flow::Stop = self.handle(command).await {
                            return;
                        }
                    }
                    _ = executor::sleep(PING_INTERVAL.saturating_sub(since_ping)) => {}
                    _ = self.closed.changed() => {
                        self.end_transport().await;
                        return;
                    }
                }
            }
            while let Ok(command) = self.commands.try_recv() {
                if let Flow::Stop = self.handle(command).await {
                    return;
                }
            }
            self.refresh();
            self.queue_ping_when_due();
            let record = self.build_record();
            if record.is_empty() {
                continue;
            }
            if let Flow::Stop = self.send_record(record).await {
                return;
            }
        }
    }

    fn has_work(&self) -> bool {
        if !self.control.is_empty() || !self.datagrams.is_empty() {
            return true;
        }
        self.peer.is_some()
            && self
                .streams
                .values()
                .any(|stream| stream.sendable(self.sent_total, self.peer_max_data))
    }

    async fn handle(&mut self, command: Command) -> Flow {
        match command {
            Command::PeerParameters(parameters) => {
                self.peer = Some(parameters);
            }
            Command::OpenSendStream { stream_id, quota } => {
                quota.raise_peer_limit(self.initial_credit(stream_id));
                self.streams.insert(
                    stream_id,
                    SendStreamState {
                        queue: VecDeque::new(),
                        queued_bytes: 0,
                        offset: 0,
                        max_stream_data: quota.peer_limit(),
                        fin_pending: false,
                        priority: 0,
                        last_served: 0,
                        quota,
                        stream_data_blocked_reported: false,
                    },
                );
            }
            Command::Data { stream_id, data } => {
                if let Some(stream) = self.streams.get_mut(&stream_id) {
                    stream.queued_bytes += data.len();
                    stream.queue.push_back(data);
                }
            }
            Command::Finish { stream_id } => {
                if let Some(stream) = self.streams.get_mut(&stream_id) {
                    stream.fin_pending = true;
                }
            }
            Command::Reset {
                stream_id,
                error_code,
            } => self.reset_stream(stream_id, error_code),
            Command::SetPriority {
                stream_id,
                priority,
            } => {
                if let Some(stream) = self.streams.get_mut(&stream_id) {
                    stream.priority = priority;
                }
            }
            Command::Datagram(data) => {
                self.datagram_bytes += data.len();
                self.datagrams.push_back(data);
                while self.datagram_bytes > DATAGRAM_SEND_BUFFER {
                    let dropped = self
                        .datagrams
                        .pop_front()
                        .expect("the buffer is over its size");
                    self.datagram_bytes -= dropped.len();
                }
            }
            Command::Frame(frame) => self.control.push_back(frame),
            Command::Wake => {}
            Command::Close(close) => {
                let reason = match &close {
                    ConnectionClose::Application { reason, .. }
                    | ConnectionClose::Transport { reason, .. } => reason.clone(),
                };
                let mut record = BytesMut::new();
                Frame::ConnectionClose(close).encode(&mut record);
                let _ = executor::timeout(CLOSE_TIMEOUT, async {
                    let _ = self.sink.send(record.freeze()).await;
                    let _ = self.sink.close(&reason).await;
                })
                .await;
                return Flow::Stop;
            }
            Command::Stop => {
                self.end_transport().await;
                return Flow::Stop;
            }
        }
        Flow::Continue
    }

    /// Takes in what the receive task and the handles wrote to the shared
    /// state since the last pass. The wake flag is cleared first, so a
    /// change after this read sends a new `Wake`.
    fn refresh(&mut self) {
        if !self.session.wake_pending.swap(false, Ordering::SeqCst) {
            return;
        }
        let peer_max_data = self.session.peer_max_data.load(Ordering::Acquire);
        if peer_max_data > self.peer_max_data {
            self.peer_max_data = peer_max_data;
            self.data_blocked_reported = false;
        }
        let mut stopped = Vec::new();
        for (stream_id, stream) in &mut self.streams {
            let limit = stream.quota.peer_limit();
            if limit > stream.max_stream_data {
                stream.max_stream_data = limit;
                stream.stream_data_blocked_reported = false;
            }
            if let Some(error_code) = stream.quota.stopped_code() {
                stopped.push((*stream_id, error_code));
            }
        }
        // RFC 9000 §3.5: answer STOP_SENDING with RESET_STREAM.
        for (stream_id, error_code) in stopped {
            self.reset_stream(stream_id, error_code);
        }
        if let Some(sequence) = self
            .session
            .pending_ping_response
            .lock()
            .expect("ping lock")
            .take()
        {
            self.control.push_back(Frame::PingResponse { sequence });
        }
        self.grant_released_streams(StreamKind::Bidirectional);
        self.grant_released_streams(StreamKind::Unidirectional);
    }

    /// MAX_STREAMS for the peer-opened streams the application has released.
    fn grant_released_streams(&mut self, kind: StreamKind) {
        let released = self.session.released_peer_streams(kind);
        let (granted, initial) = match kind {
            StreamKind::Bidirectional => (
                &mut self.granted_bidirectional,
                self.session.local.initial_max_streams_bidi,
            ),
            StreamKind::Unidirectional => (
                &mut self.granted_unidirectional,
                self.session.local.initial_max_streams_uni,
            ),
        };
        if released <= *granted {
            return;
        }
        *granted = released;
        self.control.push_back(Frame::MaxStreams {
            bidirectional: kind == StreamKind::Bidirectional,
            maximum: initial + released,
        });
    }

    /// Ends the transport after the session closed for another reason.
    async fn end_transport(&mut self) {
        let _ = executor::timeout(CLOSE_TIMEOUT, self.sink.close("")).await;
    }

    fn reset_stream(&mut self, stream_id: u64, error_code: u64) {
        let Some(stream) = self.streams.remove(&stream_id) else {
            return;
        };
        self.session
            .send_quotas
            .lock()
            .expect("send quotas lock")
            .remove(&stream_id);
        stream.quota.release(stream.queued_bytes);
        self.control.push_back(Frame::ResetStream {
            stream_id,
            error_code,
            final_size: stream.offset,
        });
    }

    /// The flow control credit a new stream starts with (RFC 9000 §18.2): the
    /// peer's limit for the streams it opened or for the ones this side opened.
    fn initial_credit(&self, stream_id: u64) -> u64 {
        let Some(peer) = &self.peer else { return 0 };
        match (
            self.session.role.opened_by_me(stream_id),
            StreamKind::of(stream_id),
        ) {
            (true, StreamKind::Bidirectional) => peer.initial_max_stream_data_bidi_remote,
            (false, StreamKind::Bidirectional) => peer.initial_max_stream_data_bidi_local,
            (true, StreamKind::Unidirectional) => peer.initial_max_stream_data_uni,
            (false, StreamKind::Unidirectional) => 0,
        }
    }

    fn queue_ping_when_due(&mut self) {
        let now = executor::now();
        if now.saturating_sub(self.last_ping_at) < PING_INTERVAL {
            return;
        }
        // Advanced before the peer's parameters too, or the idle sleep above
        // would be zero and spin until they arrive.
        self.last_ping_at = now;
        if self.peer.is_none() {
            return;
        }
        let sequence = self.next_ping;
        self.next_ping += 1;
        *self.session.ping_in_flight.lock().expect("ping lock") = Some((sequence, now));
        self.control.push_back(Frame::Ping { sequence });
    }

    fn max_record_size(&self) -> usize {
        self.peer
            .as_ref()
            .map(|peer| peer.max_record_size)
            .unwrap_or(DEFAULT_MAX_RECORD_SIZE) as usize
    }

    fn build_record(&mut self) -> BytesMut {
        let max = self.max_record_size();
        let mut record = BytesMut::new();
        while let Some(frame) = self.control.front() {
            if record.len() + frame.encoded_len() > max {
                break;
            }
            frame.encode(&mut record);
            self.control.pop_front();
        }
        while let Some(data) = self.datagrams.front() {
            if record.len() + data.len() + datagram_frame_overhead(data.len()) > max {
                break;
            }
            let data = self.datagrams.pop_front().expect("front exists");
            self.datagram_bytes -= data.len();
            Frame::Datagram { data }.encode(&mut record);
        }
        if self.peer.is_some() {
            self.append_stream_data(&mut record, max);
        }
        record
    }

    fn append_stream_data(&mut self, record: &mut BytesMut, max: usize) {
        'streams: while let Some(stream_id) = self.pick_stream() {
            // One STREAM frame per queued chunk, staying on the stream while
            // it has data, credit and room, so the scan in pick_stream runs
            // once per stream rather than once per chunk.
            loop {
                let space = max.saturating_sub(record.len());
                let stream = self
                    .streams
                    .get_mut(&stream_id)
                    .expect("picked stream exists");
                if !stream.sendable(self.sent_total, self.peer_max_data) {
                    break;
                }
                let available = stream.queue.front().map_or(0, |chunk| chunk.len() as u64);
                let credit = (stream.max_stream_data - stream.offset)
                    .min(self.peer_max_data - self.sent_total);
                let mut length = available.min(credit) as usize;
                let overhead = stream_frame_overhead(stream_id, stream.offset, length);
                if space <= overhead {
                    break 'streams;
                }
                length = length.min(space - overhead);
                let data = match stream.queue.front_mut() {
                    Some(chunk) if length > 0 => chunk.split_to(length),
                    _ => Bytes::new(),
                };
                if stream.queue.front().is_some_and(Bytes::is_empty) {
                    stream.queue.pop_front();
                }
                stream.queued_bytes -= length;
                let fin = stream.fin_pending && stream.queue.is_empty();
                Frame::Stream {
                    stream_id,
                    offset: stream.offset,
                    data,
                    fin,
                }
                .encode(record);
                stream.offset += length as u64;
                self.sent_total += length as u64;
                self.served += 1;
                stream.last_served = self.served;
                stream.quota.release(length);
                if fin {
                    self.streams.remove(&stream_id);
                    self.session
                        .send_quotas
                        .lock()
                        .expect("send quotas lock")
                        .remove(&stream_id);
                    break;
                }
            }
        }
        self.report_blocked();
    }

    /// The highest-priority stream with data and credit; equal priorities
    /// take turns.
    fn pick_stream(&self) -> Option<u64> {
        self.streams
            .iter()
            .filter(|(_, stream)| stream.sendable(self.sent_total, self.peer_max_data))
            .max_by_key(|(_, stream)| (stream.priority, std::cmp::Reverse(stream.last_served)))
            .map(|(stream_id, _)| *stream_id)
    }

    /// RFC 9000 §4.1: tell the peer once when credit, not data, is what stops a stream.
    fn report_blocked(&mut self) {
        let mut frames = Vec::new();
        let mut data_blocked = false;
        for (stream_id, stream) in &mut self.streams {
            if stream.queue.is_empty() {
                continue;
            }
            if stream.offset >= stream.max_stream_data && !stream.stream_data_blocked_reported {
                stream.stream_data_blocked_reported = true;
                frames.push(Frame::StreamDataBlocked {
                    stream_id: *stream_id,
                    limit: stream.max_stream_data,
                });
            } else if self.sent_total >= self.peer_max_data {
                data_blocked = true;
            }
        }
        Stats::add(
            &self.session.stats.sent_stream_data_blocked,
            frames.len() as u64,
        );
        self.control.extend(frames);
        if data_blocked && !self.data_blocked_reported {
            self.data_blocked_reported = true;
            Stats::add(&self.session.stats.sent_data_blocked, 1);
            self.control.push_back(Frame::DataBlocked {
                limit: self.peer_max_data,
            });
        }
    }

    async fn send_record(&mut self, record: BytesMut) -> Flow {
        let length = record.len() as u64;
        // A peer that stops reading its socket but keeps pinging would
        // otherwise block the sink, and with it every later command.
        tokio::select! {
            result = executor::timeout(IDLE_TIMEOUT, self.sink.send(record.freeze())) => match result {
                Ok(Ok(())) => {
                    Stats::add(&self.session.stats.sent_bytes, length);
                    Flow::Continue
                }
                Ok(Err(error)) => {
                    self.session.set_closed(TransportClose {
                        code: None,
                        reason: format!("transport send failed: {error}"),
                    });
                    Flow::Stop
                }
                Err(()) => {
                    self.session.set_closed(TransportClose {
                        code: None,
                        reason: format!("transport send stalled for {} s", IDLE_TIMEOUT.as_secs()),
                    });
                    Flow::Stop
                }
            },
            // Another task closed the session while the sink was blocked; it
            // may have queued the CONNECTION_CLOSE that tells the peer why.
            _ = self.closed.changed() => {
                while let Ok(command) = self.commands.try_recv() {
                    if let Command::Close(_) = command {
                        return self.handle(command).await;
                    }
                }
                self.end_transport().await;
                Flow::Stop
            }
        }
    }
}
