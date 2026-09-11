use crate::modules::moqt::data_plane::codec::control_message_decoder::ControlMessageDecoder;
use crate::modules::moqt::data_plane::stream::bi_stream_sender::BiStreamSender;
use crate::modules::moqt::data_plane::stream::stream_receiver::BiStreamReceiver;
use crate::modules::moqt::domains::session::Session;
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::domains::session_context_factory::SessionContextFactory;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::transport::connect_target::ConnectTarget;
use crate::modules::transport::transport_connection::TransportConnection;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use std::sync::atomic::AtomicU64;

use crate::{Accepting, Connecting, Handshake};

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
    pub(crate) authorization_token: Option<String>,
}

impl<T: TransportProtocol> SessionCreator<T> {
    pub(crate) async fn create_new_connection(&self, url: &str) -> anyhow::Result<Connecting<T>> {
        let target = ConnectTarget::parse(url)?;
        let transport_conn = self.transport_creator.create_new_transport(&target).await?;
        let authorization_token = self.authorization_token.clone();
        let handshake = async move {
            let (send_stream, receive_stream) = transport_conn.open_bi().await?;
            let mut send_stream = BiStreamSender::new(send_stream);
            let mut receive_stream = BiStreamReceiver::new(receive_stream, ControlMessageDecoder);
            SessionContextFactory::send_client_setup(
                &mut send_stream,
                authorization_token.as_deref(),
            )
            .await?;
            SessionContextFactory::receive_server_setup(&mut receive_stream).await?;
            let (event_sender, event_receiver) = tokio::sync::mpsc::unbounded_channel();
            let context =
                SessionContext::new(transport_conn, send_stream, AtomicU64::new(1), event_sender);
            tracing::info!("Session is created.");
            Ok(Session::<T>::new(receive_stream, context, event_receiver))
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

#[cfg(test)]
mod tests {
    use crate::{
        ClientConfig,
        modules::{
            moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken,
            test_support::{
                HANDSHAKE_TIMEOUT, dual_client, dual_client_with_config,
                spawn_dual_server_handshake,
            },
        },
    };

    #[tokio::test]
    async fn connect_sends_authorization_token_in_client_setup() {
        // Arrange
        let (port, accept) = spawn_dual_server_handshake("client-setup-with-token");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move {
            dual_client_with_config(ClientConfig {
                port: 0,
                verify_certificate: false,
                authorization_token: Some("jwt".to_string()),
            })
            .connect(&url)
            .await?
            .await
        });

        // Act
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(
            handshake
                .client_setup()
                .setup_parameters
                .authorization_token,
            vec![AuthorizationToken::use_value_utf8("jwt")]
        );
        client.abort();
    }

    #[tokio::test]
    async fn connect_without_token_sends_no_authorization_token() {
        // Arrange
        let (port, accept) = spawn_dual_server_handshake("client-setup-without-token");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { dual_client().connect(&url).await?.await });

        // Act
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert!(
            handshake
                .client_setup()
                .setup_parameters
                .authorization_token
                .is_empty()
        );
        client.abort();
    }
}
