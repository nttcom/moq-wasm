pub(crate) mod datagram_sender;
pub(crate) mod fetch_sender;
pub(crate) mod stream_sender;
pub(crate) mod stream_sender_factory;

use moqt::object::{DataStreamResetCode, StreamPriority, SubgroupPart};

#[async_trait::async_trait]
pub(crate) trait DataSender: 'static + Send + Sync {
    async fn send_object(&mut self, object: SubgroupPart) -> anyhow::Result<()>;

    async fn set_priority(&mut self, priority: StreamPriority) -> anyhow::Result<()>;

    async fn close(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn reset(&mut self, _error_code: DataStreamResetCode) -> anyhow::Result<()> {
        Ok(())
    }

    /// Resolves once a closed sender holds nothing the transport may still
    /// send (see `moqt::object::StreamDataSender::acknowledged`); at once for a
    /// sender without a stream.
    async fn acknowledged(&mut self) {}
}
