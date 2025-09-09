use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;

#[automock]
#[async_trait]
pub(crate) trait TransportSendStream: Send + Sync + 'static {
    async fn send(&mut self, buffer: &BytesMut) -> anyhow::Result<()>;
}
