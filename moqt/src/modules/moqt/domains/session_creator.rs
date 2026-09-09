use crate::modules::moqt::data_plane::codec::control_message_decoder::ControlMessageDecoder;
use crate::modules::moqt::data_plane::stream::bi_stream_sender::BiStreamSender;
use crate::modules::moqt::data_plane::stream::stream_receiver::BiStreamReceiver;
use crate::modules::moqt::domains::session::Session;
use crate::modules::moqt::domains::session_context_factory::SessionContextFactory;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::transport::connect_target::ConnectTarget;
use crate::modules::transport::transport_connection::TransportConnection;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use crate::{Accepting, Connecting, Handshake};

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
}

impl<T: TransportProtocol> SessionCreator<T> {
    pub(crate) async fn create_new_connection(&self, url: &str) -> anyhow::Result<Connecting<T>> {
        let target = ConnectTarget::parse(url)?;
        let transport_conn = self.transport_creator.create_new_transport(&target).await?;
        let handshake = async move {
            let (send_stream, receive_stream) = transport_conn.open_bi().await?;
            let mut moqt_receiver = BiStreamReceiver::new(receive_stream, ControlMessageDecoder);
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let inner = SessionContextFactory::client(
                transport_conn,
                send_stream,
                &mut moqt_receiver,
                sender,
            )
            .await
            .inspect(|_| tracing::info!("Session is created."))?;
            Ok(Session::<T>::new(moqt_receiver, inner, receiver))
        };
        Ok(Connecting {
            inner: Box::pin(handshake),
        })
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Accepting<T>> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let handshake = async move {
            let (send_stream, receive_stream) = transport_conn.accept_bi().await?;
            let mut receive_stream = BiStreamReceiver::new(receive_stream, ControlMessageDecoder);
            let client_setup =
                SessionContextFactory::receive_client_setup(&mut receive_stream).await?;
            Ok(Handshake {
                client_setup,
                transport_connection: transport_conn,
                send_stream: BiStreamSender::new(send_stream),
                receive_stream,
            })
        };
        Ok(Accepting {
            inner: Box::pin(handshake),
        })
    }
}
