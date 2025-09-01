use std::sync::Arc;

use crate::modules::moqt::moqt_bi_stream::{MOQTBiStream, ReceiveEvent};
use crate::modules::moqt::moqt_connection::MOQTConnection;
use crate::modules::moqt::moqt_connection_message_controller::MOQTConnectionMessageController;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;

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
    ) -> anyhow::Result<Arc<MOQTConnection>> {
        let transport_conn = self
            .transport_creator
            .create_new_transport(server_name, port)
            .await?;
        let stream = transport_conn.open_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveEvent>(16);
        let moqt_bi_stream = Arc::new(MOQTBiStream::new(sender.clone(), stream));
        MOQTConnection::new(true, transport_conn, moqt_bi_stream, sender).await
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Arc<MOQTConnection>> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let stream = transport_conn.accept_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveEvent>(16);
        let moqt_bi_stream = Arc::new(MOQTBiStream::new(sender.clone(), stream));
        MOQTConnection::new(false, transport_conn, moqt_bi_stream, sender).await
    }
}
