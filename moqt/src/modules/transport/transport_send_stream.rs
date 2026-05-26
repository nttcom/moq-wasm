use std::fmt::Debug;

use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportSendError {
    #[error("sending stopped by peer: error {code}")]
    Stopped { code: u64 },
    #[error("invalid stop sending code: {code}")]
    InvalidStopped { code: u64 },
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

#[automock]
#[async_trait]
pub(crate) trait TransportSendStream: Send + Sync + 'static + Debug {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError>;
    async fn close(&mut self) -> Result<(), TransportSendError>;
}
