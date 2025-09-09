use std::net::SocketAddr;
use std::sync::Arc;

use crate::modules::moqt::moqt_connection::MOQTConnection;
use crate::modules::moqt::moqt_control_receiver::MOQTControlReceiver;
use crate::modules::moqt::moqt_control_sender::MOQTControlSender;
use crate::modules::moqt::moqt_enums::ReceiveEvent;
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
        remote_address: SocketAddr,
        host: &str,
    ) -> anyhow::Result<Arc<MOQTConnection>> {
        let transport_conn = self
            .transport_creator
            .create_new_transport(remote_address, host)
            .await?;
        let (send_stream, receive_stream) = transport_conn.open_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveEvent>(16);
        let moqt_sender = MOQTControlSender::new(send_stream);
        let moqt_receiver = MOQTControlReceiver::new(receive_stream, sender.clone());
        MOQTConnection::new(true, transport_conn, moqt_sender, moqt_receiver, sender).await
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Arc<MOQTConnection>> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let (send_stream, receive_stream) = transport_conn.accept_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveEvent>(16);
        let moqt_sender = MOQTControlSender::new(send_stream);
        let moqt_receiver = MOQTControlReceiver::new(receive_stream, sender.clone());
        MOQTConnection::new(true, transport_conn, moqt_sender, moqt_receiver, sender).await
    }
}
