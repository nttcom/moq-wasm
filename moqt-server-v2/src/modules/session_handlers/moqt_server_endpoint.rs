use crate::modules::session_handlers::{
    transport_connection::TransportConnection,
    transport_connection_creator::TransportConnectionCreator,
};

pub(crate) struct MOQTServerEndpoint {
    creator: Box<dyn TransportConnectionCreator>,
}

impl MOQTServerEndpoint {
    pub(crate) fn new(creator: Box<dyn TransportConnectionCreator>) -> Self {
        Self { creator }
    }
}

impl MOQTServerEndpoint {
    async fn create_new_connection(
        &mut self,
        server_name: &str,
        port: u16,
    ) -> anyhow::Result<Box<dyn TransportConnection>> {
        self.creator.create_new_connection(server_name, port).await
    }
    async fn accept_new_connection(&mut self) -> anyhow::Result<Box<dyn TransportConnection>> {
        self.creator.accept_new_connection().await
    }
}
