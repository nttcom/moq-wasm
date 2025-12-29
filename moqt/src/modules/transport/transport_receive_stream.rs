use std::fmt::Debug;

use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;

#[automock]
#[async_trait]
pub(crate) trait TransportReceiveStream: Send + Sync + 'static + Debug {
    async fn receive(&mut self, buffer: &mut BytesMut) -> anyhow::Result<Option<usize>>;
}
