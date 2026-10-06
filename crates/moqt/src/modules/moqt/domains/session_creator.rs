use crate::modules::executor;
use crate::modules::moqt::control_plane::control_messages::messages::server_setup::ServerSetup;
use crate::modules::moqt::data_plane::codec::control_message_decoder::ControlMessageDecoder;
use crate::modules::moqt::data_plane::stream::bi_stream_sender::BiStreamSender;
use crate::modules::moqt::data_plane::stream::stream_receiver::BiStreamReceiver;
use crate::modules::moqt::domains::session::Session;
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::domains::session_context_factory::SessionContextFactory;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::transport::connect_target::ConnectTarget;
use crate::modules::transport::transport_connection::BoxedConnection;
use crate::modules::transport::transport_connection::TransportClose;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use crate::{Accepting, Connecting, Handshake};

const CLOSE_REASON_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
    pub(crate) authorization_token: Option<String>,
}

impl<T: TransportProtocol> SessionCreator<T> {
    pub(crate) async fn create_new_connection(&self, url: &str) -> anyhow::Result<Connecting> {
        let target = ConnectTarget::parse(url)?;
        let transport_conn = self.transport_creator.create_new_transport(&target).await?;
        let authorization_token = self.authorization_token.clone();
        let handshake = async move {
            let (send_stream, receive_stream, server_setup) =
                match Self::exchange_setup(&transport_conn, authorization_token.as_deref()).await {
                    Ok(established) => established,
                    Err(error) => {
                        return Err(Self::explain_rejected_setup(&transport_conn, error).await);
                    }
                };
            let (event_sender, event_receiver) = tokio::sync::mpsc::unbounded_channel();
            let context =
                SessionContext::new(transport_conn, send_stream, AtomicU64::new(0), event_sender);
            tracing::info!("Session is created.");
            Ok(Session::new(
                receive_stream,
                context,
                event_receiver,
                Some(server_setup),
            ))
        };
        Ok(Connecting {
            inner: Box::pin(handshake),
        })
    }

    async fn exchange_setup(
        transport_conn: &BoxedConnection,
        authorization_token: Option<&str>,
    ) -> anyhow::Result<(BiStreamSender, BiStreamReceiver, ServerSetup)> {
        let (send_stream, receive_stream) = transport_conn.open_bi().await?;
        let mut send_stream = BiStreamSender::new(send_stream);
        let mut receive_stream = BiStreamReceiver::new(receive_stream, ControlMessageDecoder);
        SessionContextFactory::send_client_setup(&mut send_stream, authorization_token).await?;
        let server_setup = SessionContextFactory::receive_server_setup(&mut receive_stream).await?;
        Ok((send_stream, receive_stream, server_setup))
    }

    /// A peer that rejects CLIENT_SETUP closes the transport with a termination
    /// code (draft-14 §3.4). The stream error the setup exchange saw does not
    /// carry it, so the close is awaited briefly and put in front of the error.
    async fn explain_rejected_setup(
        transport_conn: &BoxedConnection,
        error: anyhow::Error,
    ) -> anyhow::Error {
        match executor::timeout(CLOSE_REASON_TIMEOUT, transport_conn.closed()).await {
            Ok(TransportClose {
                code: Some(code),
                reason,
            }) => error.context(format!(
                "peer closed the connection before SERVER_SETUP (code {code}: {reason})"
            )),
            _ => error,
        }
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<Accepting> {
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
        ClientConfig, TerminationErrorCode,
        modules::{
            moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken,
            test_support::{
                HANDSHAKE_TIMEOUT, dual_client, dual_client_with_config,
                spawn_connected_dual_sessions, spawn_dual_server_handshake,
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

    #[tokio::test]
    async fn client_request_ids_are_even_from_zero_and_server_ids_odd_from_one() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-parity").await;
        let client_context = client.publisher().session;
        let server_context = server.publisher().session;

        // Act
        let client_ids = [
            client_context.get_request_id(),
            client_context.get_request_id(),
        ];
        let server_ids = [
            server_context.get_request_id(),
            server_context.get_request_id(),
        ];

        // Assert
        assert_eq!(client_ids, [0, 2]);
        assert_eq!(server_ids, [1, 3]);
    }

    #[tokio::test]
    async fn a_rejected_client_setup_reports_the_peer_close_code_and_reason() {
        // Arrange
        let (port, accept) = spawn_dual_server_handshake("session-creator-rejected-setup");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { dual_client().connect(&url).await?.await });
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Act
        handshake
            .reject(TerminationErrorCode::Unauthorized, "bad token")
            .await;
        let Err(error) = tokio::time::timeout(HANDSHAKE_TIMEOUT, client)
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("expected the handshake to be rejected");
        };

        // Assert
        assert!(
            format!("{error:#}").contains("code 2: bad token"),
            "{error:#}"
        );
    }
}
