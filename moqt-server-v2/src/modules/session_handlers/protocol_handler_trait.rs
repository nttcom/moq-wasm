use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::modules::session_handlers::bi_stream::BiStreamTrait;

#[async_trait]
pub(crate) trait ProtocolHandlerTrait: Send + Sync {
    async fn start(&self) -> anyhow::Result<Arc<Mutex<dyn BiStreamTrait>>>;
    async fn start_listen(&self) -> anyhow::Result<()>;
    fn finish(&self) -> anyhow::Result<()>;
}
