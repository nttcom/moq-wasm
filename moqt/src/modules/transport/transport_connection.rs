use std::sync::Arc;

use crate::modules::transport::transport_bi_stream::TransportBiStream;
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransportConnection: Send + Sync {
    async fn open_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn TransportBiStream>>>;
    async fn accept_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn TransportBiStream>>>;
}
