use std::{
    fmt::Debug,
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::BytesMut;
use thiserror::Error;

use crate::modules::executor::{MaybeSend, MaybeSync};

#[derive(Debug, Error)]
pub enum TransportSendError {
    #[error("sending stopped by peer: error {code}")]
    Stopped { code: u64 },
    #[error("connection lost: {reason}")]
    ConnectionLost { reason: String },
    #[error("session error: {reason}")]
    SessionError { reason: String },
    #[error("closed stream")]
    ClosedStream,
    #[error("0-RTT rejected")]
    ZeroRttRejected,
    #[error("transport send failed: {source}")]
    Transport {
        #[source]
        source: anyhow::Error,
    },
}

impl TransportSendError {
    pub fn is_stopped_by_peer(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<TransportSendError>(),
                Some(TransportSendError::Stopped { .. })
            )
        })
    }
}

#[cfg_attr(test, mockall::automock)]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait TransportSendStream: MaybeSend + MaybeSync + 'static + Debug {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError>;
    /// Queues the FIN synchronously, so an implementation cannot wait for the peer's ACK.
    fn close(&mut self) -> Result<(), TransportSendError>;
    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError>;
    fn set_priority(&mut self, priority: i32) -> Result<(), TransportSendError>;
    /// `Ready(Some(code))` once the peer sent STOP_SENDING with `code`, and
    /// `Ready(None)` once it no longer can: the FIN was acknowledged or the
    /// connection is lost. Once this side has reset the stream, it may stay
    /// `Pending` or report the reset's code. A `Pending` poll keeps its wait,
    /// so `cx` is woken later.
    fn poll_stopped<'a>(&mut self, cx: &mut Context<'a>) -> Poll<Option<u64>>;
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn detects_stop_sending_through_context_layers() {
        // Arrange
        let stopped = Err::<(), _>(TransportSendError::Stopped { code: 0 })
            .context("send subgroup object")
            .unwrap_err();
        let lost = Err::<(), _>(TransportSendError::ConnectionLost {
            reason: "timeout".into(),
        })
        .context("send subgroup object")
        .unwrap_err();

        // Act / Assert
        assert!(TransportSendError::is_stopped_by_peer(&stopped));
        assert!(!TransportSendError::is_stopped_by_peer(&lost));
    }
}
