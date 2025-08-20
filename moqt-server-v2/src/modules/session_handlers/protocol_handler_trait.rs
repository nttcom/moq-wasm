use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::modules::session_handlers::bi_stream::BiStreamTrait;

pub(crate) enum ConnectionEvent {
    OnControlStreamAdded {stream: Box<dyn BiStreamTrait>},
    OnError {message: String}
}

#[async_trait]
pub(crate) trait ProtocolHandlerTrait: Send + Sync {
    async fn start(&mut self) -> anyhow::Result<Arc<Mutex<dyn BiStreamTrait>>>;
    fn start_listen(&mut self, event_sender: tokio::sync::mpsc::Sender::<ConnectionEvent>) -> bool;
    fn finish(&self) -> anyhow::Result<()>;
}
