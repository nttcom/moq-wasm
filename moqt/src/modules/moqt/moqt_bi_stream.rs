use async_trait::async_trait;
use mockall::automock;
use bytes::BytesMut;

#[automock]
#[async_trait]
pub(crate) trait MOQTBiStream: Send + Sync + 'static {
    fn get_stream_id(&self) -> u64;
    async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()>;
    async fn receive(&mut self) -> anyhow::Result<BytesMut>;
}
