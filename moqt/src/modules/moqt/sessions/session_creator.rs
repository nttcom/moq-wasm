use std::net::SocketAddr;
use std::sync::Arc;

use crate::modules::moqt::control_receiver::ControlReceiver;
use crate::modules::moqt::control_sender::ControlSender;
use crate::modules::moqt::moqt_enums::ReceiveEvent;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::session::Session;
use crate::modules::transport::transport_connection::TransportConnection;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
}

impl<T: TransportProtocol> SessionCreator<T> {
    pub(crate) async fn create_new_connection(
        &self,
        remote_address: SocketAddr,
        host: &str,
    ) -> anyhow::Result<Arc<Session<T>>> {
        let transport_conn = self
            .transport_creator
            .create_new_transport(remote_address, host)
            .await?;
        let (send_stream, receive_stream) = transport_conn.open_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveEvent>(16);
        let moqt_sender = ControlSender::<T> { send_stream };
        let moqt_receiver = ControlReceiver::new::<T>(receive_stream, sender.clone());
        Session::<T>::for_client(transport_conn, moqt_sender, moqt_receiver, sender)
            .await
            .inspect(|_| tracing::info!("Session has been created."))
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Arc<Session<T>>> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let (send_stream, receive_stream) = transport_conn.accept_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let (sender, _) = tokio::sync::broadcast::channel::<ReceiveEvent>(16);
        let moqt_sender = ControlSender { send_stream };
        let moqt_receiver = ControlReceiver::new::<T>(receive_stream, sender.clone());
        Session::<T>::for_server(transport_conn, moqt_sender, moqt_receiver, sender)
            .await
            .inspect(|_| tracing::info!("Session has been created."))
    }
}
