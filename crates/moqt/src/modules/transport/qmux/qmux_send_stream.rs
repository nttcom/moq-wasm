use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use tokio::sync::Notify;

use super::session_state::{Command, SessionState, wait_until};
use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

/// The bytes one stream may have queued in the send task before `send`
/// waits: the task drains queues by priority, so this, not the command
/// channel, is where backpressure reaches the application.
const STREAM_SEND_QUEUE_LIMIT: usize = 64 * 1024;

#[cfg(not(target_arch = "wasm32"))]
type Stopped = std::pin::Pin<Box<dyn Future<Output = Option<u64>> + Send + Sync>>;
#[cfg(target_arch = "wasm32")]
type Stopped = std::pin::Pin<Box<dyn Future<Output = Option<u64>>>>;

/// What a send stream handle, the receive task and the send task share
/// about one outgoing stream.
#[derive(Default)]
pub(crate) struct SendQuota {
    state: Mutex<QuotaState>,
    notify: Notify,
    /// The peer's MAX_STREAM_DATA; only ever raised.
    peer_limit: AtomicU64,
}

#[derive(Default)]
struct QuotaState {
    queued: usize,
    /// The peer's STOP_SENDING code.
    stopped: Option<u64>,
    connection_lost: bool,
}

impl SendQuota {
    /// Called by the send task once `bytes` of the stream left its queue;
    /// `stopped` may already have emptied the queue.
    pub(crate) fn release(&self, bytes: usize) {
        let mut state = self.state.lock().expect("send quota lock");
        state.queued = state.queued.saturating_sub(bytes);
        drop(state);
        self.notify.notify_waiters();
    }

    pub(crate) fn stopped(&self, error_code: u64) {
        let mut state = self.state.lock().expect("send quota lock");
        state.stopped = Some(error_code);
        state.queued = 0;
        drop(state);
        self.notify.notify_waiters();
    }

    pub(crate) fn stopped_code(&self) -> Option<u64> {
        self.state.lock().expect("send quota lock").stopped
    }

    pub(crate) fn raise_peer_limit(&self, maximum: u64) {
        self.peer_limit.fetch_max(maximum, Ordering::AcqRel);
    }

    pub(crate) fn peer_limit(&self) -> u64 {
        self.peer_limit.load(Ordering::Acquire)
    }

    pub(crate) fn connection_lost(&self) {
        self.state.lock().expect("send quota lock").connection_lost = true;
        self.notify.notify_waiters();
    }

    /// The peer's STOP_SENDING code, or `None` once the connection is lost.
    async fn stopped_or_lost(&self) -> Option<u64> {
        wait_until(&self.notify, || {
            let state = self.state.lock().expect("send quota lock");
            match (state.stopped, state.connection_lost) {
                (Some(code), _) => Some(Some(code)),
                (None, true) => Some(None),
                (None, false) => None,
            }
        })
        .await
    }

    /// Reserves up to `wanted` bytes of queue space once some is free.
    async fn reserve(&self, wanted: usize) -> Result<usize, TransportSendError> {
        wait_until(&self.notify, || {
            let mut state = self.state.lock().expect("send quota lock");
            if let Some(code) = state.stopped {
                return Some(Err(TransportSendError::Stopped { code }));
            }
            if state.connection_lost {
                return Some(Err(connection_lost()));
            }
            let free = STREAM_SEND_QUEUE_LIMIT.saturating_sub(state.queued);
            if free == 0 {
                return None;
            }
            let reserved = free.min(wanted);
            state.queued += reserved;
            Some(Ok(reserved))
        })
        .await
    }
}

pub(crate) struct QmuxSendStream {
    stream_id: u64,
    session: Arc<SessionState>,
    quota: Arc<SendQuota>,
    /// Set once `close` or `reset` ran, so `Drop` does not reset the stream.
    ended: bool,
    /// The wait `poll_stopped` keeps between polls.
    stopped: Option<Stopped>,
}

impl QmuxSendStream {
    pub(crate) fn new(stream_id: u64, session: Arc<SessionState>, quota: Arc<SendQuota>) -> Self {
        Self {
            stream_id,
            session,
            quota,
            ended: false,
            stopped: None,
        }
    }

    fn command(&self, command: Command) -> Result<(), TransportSendError> {
        self.session
            .commands
            .send(command)
            .map_err(|_| connection_lost())
    }
}

impl std::fmt::Debug for QmuxSendStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QmuxSendStream")
            .field("stream_id", &self.stream_id)
            .finish()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl TransportSendStream for QmuxSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        if self.ended {
            return Err(TransportSendError::ClosedStream);
        }
        let mut sent = 0;
        while sent < buffer.len() {
            let reserved = self.quota.reserve(buffer.len() - sent).await?;
            self.command(Command::Data {
                stream_id: self.stream_id,
                data: Bytes::copy_from_slice(&buffer[sent..sent + reserved]),
            })?;
            sent += reserved;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), TransportSendError> {
        if self.ended {
            return Err(TransportSendError::ClosedStream);
        }
        self.ended = true;
        self.command(Command::Finish {
            stream_id: self.stream_id,
        })
    }

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        if self.ended {
            return Err(TransportSendError::ClosedStream);
        }
        self.ended = true;
        self.command(Command::Reset {
            stream_id: self.stream_id,
            error_code,
        })
    }

    fn set_priority(&mut self, priority: i32) -> Result<(), TransportSendError> {
        self.command(Command::SetPriority {
            stream_id: self.stream_id,
            priority,
        })
    }

    /// The byte stream under QMux is reliable, so a FIN reaches the peer
    /// once written: after `close`, the stream can no longer be stopped.
    fn poll_stopped(&mut self, cx: &mut Context<'_>) -> Poll<Option<u64>> {
        if let Some(code) = self.quota.stopped_code() {
            return Poll::Ready(Some(code));
        }
        if self.ended {
            return Poll::Ready(None);
        }
        let stopped = self.stopped.get_or_insert_with(|| {
            let quota = self.quota.clone();
            Box::pin(async move { quota.stopped_or_lost().await })
        });
        let polled = stopped.as_mut().poll(cx);
        if polled.is_ready() {
            self.stopped = None;
        }
        polled
    }
}

/// As in quinn, a stream dropped before `close` or `reset` is reset with
/// code 0, so the peer does not wait for data that never comes.
impl Drop for QmuxSendStream {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.command(Command::Reset {
                stream_id: self.stream_id,
                error_code: 0,
            });
        }
    }
}

fn connection_lost() -> TransportSendError {
    TransportSendError::ConnectionLost {
        reason: "QMux session closed".into(),
    }
}
