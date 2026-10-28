use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use tokio::sync::{Notify, mpsc, watch};

use super::{
    frame::{ConnectionClose, Frame},
    qmux_receive_stream::ReceiveShared,
    qmux_send_stream::SendQuota,
    stats::Stats,
    stream_id::{Role, StreamKind},
    transport_parameters::TransportParameters,
};
use crate::modules::transport::transport_connection::TransportClose;

/// What the send task is told to do. Handles and the receive task send
/// these; only the send task touches the message sink.
pub(crate) enum Command {
    PeerParameters(TransportParameters),
    OpenSendStream {
        stream_id: u64,
        quota: Arc<SendQuota>,
    },
    Data {
        stream_id: u64,
        data: Bytes,
    },
    Finish {
        stream_id: u64,
    },
    Reset {
        stream_id: u64,
        error_code: u64,
    },
    SetPriority {
        stream_id: u64,
        priority: i32,
    },
    Datagram(Bytes),
    /// A frame to send as is: this side's flow control grants and STOP_SENDING.
    Frame(Frame),
    /// Something changed in the shared state; sent at most once per pass of
    /// the send task, so the peer cannot queue work here.
    Wake,
    /// Sends CONNECTION_CLOSE and ends the transport.
    Close(ConnectionClose),
    /// Ends the transport without a frame: the peer already closed it.
    Stop,
}

/// The state a session's handles and tasks share.
pub(crate) struct SessionState {
    pub(crate) role: Role,
    pub(crate) local: TransportParameters,
    pub(crate) commands: mpsc::UnboundedSender<Command>,
    pub(crate) stats: Stats,
    closed: watch::Sender<Option<TransportClose>>,
    pub(crate) peer_parameters: watch::Sender<Option<TransportParameters>>,
    opening: Mutex<Opening>,
    pub(crate) opening_notify: Notify,
    pub(crate) receive_streams: Mutex<HashMap<u64, Arc<ReceiveShared>>>,
    pub(crate) send_quotas: Mutex<HashMap<u64, Arc<SendQuota>>>,
    /// Bytes the application has read across all streams.
    consumed: AtomicU64,
    /// The MAX_DATA value the peer currently knows.
    advertised: AtomicU64,
    /// The latest QX_PING sent and when, for the RTT.
    pub(crate) ping_in_flight: Mutex<Option<(u64, Duration)>>,
    /// The largest QX_PING sequence the peer sent that is not yet answered.
    pub(crate) pending_ping_response: Mutex<Option<u64>>,
    /// The peer's MAX_DATA; only ever raised.
    pub(crate) peer_max_data: AtomicU64,
    /// Peer-opened streams whose handle the application dropped, per kind;
    /// each one lets the peer open another (MAX_STREAMS).
    released_bidirectional: AtomicU64,
    released_unidirectional: AtomicU64,
    pub(crate) wake_pending: AtomicBool,
    /// Set by `TransportEnded`.
    transport_ended: watch::Sender<bool>,
}

/// The streams this side opens: the next index per kind and the peer's limit.
struct Opening {
    next_bidi: u64,
    next_uni: u64,
    peer_max_bidi: u64,
    peer_max_uni: u64,
}

impl SessionState {
    pub(crate) fn new(
        role: Role,
        local: TransportParameters,
        commands: mpsc::UnboundedSender<Command>,
    ) -> Self {
        Self {
            role,
            advertised: AtomicU64::new(local.initial_max_data),
            local,
            commands,
            stats: Stats::default(),
            closed: watch::Sender::new(None),
            peer_parameters: watch::Sender::new(None),
            opening: Mutex::new(Opening {
                next_bidi: 0,
                next_uni: 0,
                peer_max_bidi: 0,
                peer_max_uni: 0,
            }),
            opening_notify: Notify::new(),
            receive_streams: Mutex::new(HashMap::new()),
            send_quotas: Mutex::new(HashMap::new()),
            consumed: AtomicU64::new(0),
            ping_in_flight: Mutex::new(None),
            pending_ping_response: Mutex::new(None),
            peer_max_data: AtomicU64::new(0),
            released_bidirectional: AtomicU64::new(0),
            released_unidirectional: AtomicU64::new(0),
            wake_pending: AtomicBool::new(false),
            transport_ended: watch::Sender::new(false),
        }
    }

    pub(crate) fn transport_ended(&self) -> watch::Receiver<bool> {
        self.transport_ended.subscribe()
    }

    pub(crate) fn mark_transport_ended(&self) {
        self.transport_ended.send_replace(true);
    }

    /// Closes from this side: the send task writes `close` as the last
    /// frame, and every handle learns at once that the session is over.
    /// The command goes first so that the send task, which prefers
    /// commands over the closed watch, finds it.
    pub(crate) fn close(&self, close: ConnectionClose, reason: String) {
        let _ = self.commands.send(Command::Close(close));
        self.set_closed(TransportClose { code: None, reason });
    }

    /// Has the send task read the shared state again.
    pub(crate) fn wake(&self) {
        if !self.wake_pending.swap(true, Ordering::SeqCst) {
            let _ = self.commands.send(Command::Wake);
        }
    }

    pub(crate) fn release_peer_stream(&self, kind: StreamKind) {
        self.released(kind).fetch_add(1, Ordering::AcqRel);
        self.wake();
    }

    pub(crate) fn released_peer_streams(&self, kind: StreamKind) -> u64 {
        self.released(kind).load(Ordering::Acquire)
    }

    fn released(&self, kind: StreamKind) -> &AtomicU64 {
        match kind {
            StreamKind::Bidirectional => &self.released_bidirectional,
            StreamKind::Unidirectional => &self.released_unidirectional,
        }
    }

    pub(crate) fn closed(&self) -> watch::Receiver<Option<TransportClose>> {
        self.closed.subscribe()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.borrow().is_some()
    }

    /// Records how the session ended, once; later calls keep the first reason.
    /// Every waiting handle is woken with the connection lost.
    pub(crate) fn set_closed(&self, close: TransportClose) {
        let first = self.closed.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(close);
            true
        });
        if !first {
            return;
        }
        for shared in self
            .receive_streams
            .lock()
            .expect("receive streams lock")
            .values()
        {
            shared.connection_lost();
        }
        for quota in self.send_quotas.lock().expect("send quotas lock").values() {
            quota.connection_lost();
        }
        self.opening_notify.notify_waiters();
    }

    pub(crate) fn set_peer_max_streams(&self, kind: StreamKind, maximum: u64) {
        let mut opening = self.opening.lock().expect("opening lock");
        let current = match kind {
            StreamKind::Bidirectional => &mut opening.peer_max_bidi,
            StreamKind::Unidirectional => &mut opening.peer_max_uni,
        };
        *current = (*current).max(maximum);
        drop(opening);
        self.opening_notify.notify_waiters();
    }

    /// The id of a new stream of `kind`, once the peer's MAX_STREAMS allows one.
    pub(crate) async fn allocate_stream_id(&self, kind: StreamKind) -> anyhow::Result<u64> {
        wait_until(&self.opening_notify, || {
            if self.is_closed() {
                return Some(Err(anyhow::anyhow!("QMux session closed")));
            }
            let mut opening = self.opening.lock().expect("opening lock");
            let limit = match kind {
                StreamKind::Bidirectional => opening.peer_max_bidi,
                StreamKind::Unidirectional => opening.peer_max_uni,
            };
            let next = match kind {
                StreamKind::Bidirectional => &mut opening.next_bidi,
                StreamKind::Unidirectional => &mut opening.next_uni,
            };
            if *next >= limit {
                return None;
            }
            let index = *next;
            *next += 1;
            Some(Ok(self.role.stream_id(kind, index)))
        })
        .await
    }

    pub(crate) fn register_send_stream(&self, stream_id: u64) -> Arc<SendQuota> {
        let quota = Arc::new(SendQuota::default());
        self.send_quotas
            .lock()
            .expect("send quotas lock")
            .insert(stream_id, quota.clone());
        // set_closed may have run between the caller's check and this insert,
        // so this quota missed its connection_lost.
        if self.is_closed() {
            quota.connection_lost();
        }
        let _ = self.commands.send(Command::OpenSendStream {
            stream_id,
            quota: quota.clone(),
        });
        quota
    }

    pub(crate) fn register_receive_stream(
        &self,
        stream_id: u64,
        window: u64,
    ) -> Arc<ReceiveShared> {
        let shared = Arc::new(ReceiveShared::new(window));
        self.receive_streams
            .lock()
            .expect("receive streams lock")
            .insert(stream_id, shared.clone());
        if self.is_closed() {
            shared.connection_lost();
        }
        shared
    }

    /// The application read `bytes` more; MAX_DATA is raised once half the
    /// connection window has been read.
    pub(crate) fn consumed(&self, bytes: u64) {
        let consumed = self.consumed.fetch_add(bytes, Ordering::AcqRel) + bytes;
        let window = self.local.initial_max_data;
        let advertised = self.advertised.load(Ordering::Acquire);
        if advertised - consumed.min(advertised) > window / 2 {
            return;
        }
        let maximum = consumed + window;
        if self
            .advertised
            .compare_exchange(advertised, maximum, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = self
                .commands
                .send(Command::Frame(Frame::MaxData { maximum }));
        }
    }

    pub(crate) fn connection_window_end(&self) -> u64 {
        self.advertised.load(Ordering::Acquire)
    }
}

/// Marks the transport ended once both tasks have returned, whichever path
/// each takes: the send task has written the last record and WebSocket Close,
/// and the receive task has stopped draining, so the socket is not closed
/// with unread bytes (which would answer with RST).
pub(crate) struct TransportEnded(pub(crate) Arc<SessionState>);

impl Drop for TransportEnded {
    fn drop(&mut self) {
        self.0.mark_transport_ended();
    }
}

/// Resolves once `check` returns `Some`, re-checking after every
/// `notify_waiters()` on `notify`; the `Notified` is armed before the check,
/// so a notification between the check and the await is not lost.
pub(crate) async fn wait_until<T>(notify: &Notify, mut check: impl FnMut() -> Option<T>) -> T {
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(value) = check() {
            return value;
        }
        notified.await;
    }
}
