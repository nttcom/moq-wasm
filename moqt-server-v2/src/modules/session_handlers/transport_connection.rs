use std::sync::Arc;

use async_trait::async_trait;

use crate::modules::session_handlers::moqt_bi_stream::MOQTBiStream;

#[async_trait]
pub(crate) trait TransportConnection: Send + Sync {
    async fn accept_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn MOQTBiStream>>>;
}
