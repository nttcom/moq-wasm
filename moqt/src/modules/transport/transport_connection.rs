use std::sync::Arc;

use async_trait::async_trait;

use crate::modules::moqt::moqt_bi_stream::MOQTBiStream;

#[async_trait]
pub(crate) trait TransportConnection: Send + Sync {
    async fn open_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn MOQTBiStream>>>;
    async fn accept_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn MOQTBiStream>>>;
}
