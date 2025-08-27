use crate::modules::session_handlers::moqt_connection::MOQTConnection;
use crate::modules::session_handlers::transport_connection_creator::TransportConnectionCreator;

pub(crate) struct MOQTConnectionCreator {
    transport_creator: Box<dyn TransportConnectionCreator>,
}

impl MOQTConnectionCreator {
    pub(crate) fn new(transport_creator: Box<dyn TransportConnectionCreator>) -> Self {
        Self { transport_creator }
    }

    pub(crate) async fn create_new_connection(
        &self,
        server_name: &str,
        port: u16,
    ) -> anyhow::Result<MOQTConnection> {
        let transport_conn = self
            .transport_creator
            .create_new_connection(server_name, port)
            .await?;
        let stream = transport_conn.accept_bi().await?;
    }

    pub(crate) async fn accept_new_connection(&self) -> anyhow::Result<MOQTConnection> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let stream = transport_conn.accept_bi().await?;
    }
}
