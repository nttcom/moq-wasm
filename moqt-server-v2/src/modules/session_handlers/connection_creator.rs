use async_trait::async_trait;

use crate::modules::session_handlers::connection_trait::Connection;

#[async_trait]
pub(crate) trait ConnectionCreator: Send + Sync {
    async fn accept_new_connection(&mut self) -> anyhow::Result<Box<dyn Connection>>;
}
