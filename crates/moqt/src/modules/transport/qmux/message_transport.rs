use async_trait::async_trait;
use bytes::Bytes;

use crate::modules::executor::MaybeSend;

/// The sending half of the message transport under a QMux session: each
/// message is the `Frames` field of one QMux record.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait MessageSink: MaybeSend + 'static {
    async fn send(&mut self, message: Bytes) -> anyhow::Result<()>;
    /// Ends the transport after the last message, telling the peer `reason`.
    async fn close(&mut self, reason: &str) -> anyhow::Result<()>;
}

/// The receiving half of the message transport under a QMux session.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait MessageSource: MaybeSend + 'static {
    /// `Ok(None)` once the peer has ended the transport.
    async fn receive(&mut self) -> anyhow::Result<Option<Bytes>>;
}
