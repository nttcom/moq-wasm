pub(crate) mod datagram_sender;
pub(crate) mod fetch_sender;
pub(crate) mod stream_sender;
pub(crate) mod stream_sender_factory;

use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait DataSender: 'static + Send + Sync {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()>;

    /// Applies MoQT priorities to the underlying transport stream.
    /// No-op for senders without a per-stream priority (e.g. datagrams).
    async fn set_priority(
        &mut self,
        _subscriber_priority: u8,
        _publisher_priority: u8,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
