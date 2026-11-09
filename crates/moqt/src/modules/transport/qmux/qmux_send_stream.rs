use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
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
}

impl QmuxSendStream {
    pub(crate) fn new(stream_id: u64, session: Arc<SessionState>, quota: Arc<SendQuota>) -> Self {
        Self {
            stream_id,
            session,
            quota,
            ended: false,
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
