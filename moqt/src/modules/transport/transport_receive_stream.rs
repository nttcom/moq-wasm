use std::fmt::Debug;

use async_trait::async_trait;
use mockall::automock;

#[automock]
#[async_trait]
pub(crate) trait TransportReceiveStream: Send + Sync + 'static + Debug {
    async fn receive(&mut self, buffer: &mut Vec<u8>) -> anyhow::Result<Option<usize>>;
}
