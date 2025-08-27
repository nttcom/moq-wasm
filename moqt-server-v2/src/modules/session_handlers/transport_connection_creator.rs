use crate::modules::session_handlers::transport_connection::TransportConnection;
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransportConnectionCreator: Send + Sync + 'static {
    async fn create_new_transport(
        &self,
        server_name: &str,
        port: u16,
    ) -> anyhow::Result<Box<dyn TransportConnection>>;
    async fn accept_new_transport(&mut self) -> anyhow::Result<Box<dyn TransportConnection>>;
}
