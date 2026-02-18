pub(crate) mod datagram_sender;
pub(crate) mod stream_sender;

use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait DataSender: 'static + Send + Sync {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()>;
}
