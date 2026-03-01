use std::net::SocketAddr;

use crate::modules::moqt::data_plane::codec::message_decoder::MessageDecoder;
use crate::modules::moqt::data_plane::streams::stream::stream_receiver::BiStreamReceiver;
use crate::modules::moqt::domains::session::Session;
use crate::modules::moqt::domains::session_context_factory::SessionContextFactory;
use crate::modules::moqt::protocol::TransportProtocol;
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
        let mut moqt_receiver = BiStreamReceiver::new(receive_stream, MessageDecoder);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let inner =
            SessionContextFactory::client(transport_conn, send_stream, &mut moqt_receiver, sender)
                .await
                .inspect(|_| tracing::info!("Session is created."))?;
        Ok(Session::<T>::new(moqt_receiver, inner, receiver))
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Session<T>> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let (send_stream, receive_stream) = transport_conn.accept_bi().await?;
        // 16 means the number of messages can be stored in the channel.
        let mut moqt_receiver = BiStreamReceiver::new(receive_stream, MessageDecoder);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let inner =
            SessionContextFactory::server(transport_conn, send_stream, &mut moqt_receiver, sender)
                .await
                .inspect(|_| tracing::info!("Session is established."))?;
        Ok(Session::<T>::new(moqt_receiver, inner, receiver))
    }
}
