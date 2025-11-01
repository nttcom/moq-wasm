use std::net::SocketAddr;

use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::session::Session;
use crate::modules::moqt::sessions::session_context::SessionContext;
use crate::modules::moqt::streams::stream::stream_receiver::StreamReceiver;
use crate::modules::moqt::streams::stream::stream_sender::StreamSender;
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
    ) -> anyhow::Result<Session<T>> {
        let transport_conn = self
            .transport_creator
            .create_new_transport(remote_address, host)
            .await?;
        let (send_stream, receive_stream) = transport_conn.open_bi().await?;
        let moqt_sender = StreamSender::<T>::new(send_stream);
        let moqt_receiver = StreamReceiver::<T>::new(receive_stream);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let inner = SessionContext::<T>::client(transport_conn, moqt_sender, moqt_receiver, sender)
            .await
            .inspect(|_| tracing::info!("Session has been created."))?;
        Ok(Session::<T>::new(inner, receiver))
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Session<T>> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let (send_stream, receive_stream) = transport_conn.accept_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let moqt_sender = StreamSender::<T>::new(send_stream);
        let moqt_receiver = StreamReceiver::<T>::new(receive_stream);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let inner = SessionContext::<T>::server(transport_conn, moqt_sender, moqt_receiver, sender)
            .await
            .inspect(|_| tracing::info!("Session has been established."))?;
        Ok(Session::<T>::new(inner, receiver))
    }
}
