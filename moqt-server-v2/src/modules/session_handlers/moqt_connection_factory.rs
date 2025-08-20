use async_trait::async_trait;

use crate::modules::session_handlers::moqt_connection::MOQTConnection;

#[async_trait]
pub(crate) trait MOQTConnectionFactory: Send + Sync {
    async fn accept_new_connection(&mut self) -> anyhow::Result<Box<dyn MOQTConnection>>;
}
