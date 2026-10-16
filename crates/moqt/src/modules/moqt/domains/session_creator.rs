use std::sync::Arc;

use crate::SessionEvent;
use crate::modules::executor::{self, BoxFuture, MaybeSend, MaybeSync};
use crate::modules::moqt::control_plane::control_messages::messages::server_setup::ServerSetup;
use crate::modules::moqt::domains::connection::Connection;
use crate::modules::moqt::domains::session::{Follow, Session};
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::version::{self, ControlReceiver};
use crate::modules::transport::accept_error::AcceptError;
use crate::modules::transport::connect_target::ConnectTarget;
use crate::modules::transport::transport_connection::BoxedConnection;
use crate::modules::transport::transport_connection::TransportClose;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use std::time::Duration;

use crate::{Connecting, server::Accepting, server::Handshake};

const CLOSE_REASON_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
    pub(crate) authorization_token: Option<String>,
    pub(crate) follow_go_away: bool,
}

/// Opens another connection of a session that follows GOAWAY, on the same
/// endpoint and transport as the first.
pub(crate) trait Reconnect: MaybeSend + MaybeSync {
    fn connect(
        &self,
        url: String,
        generation: u64,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> BoxFuture<anyhow::Result<(Connection, ControlReceiver)>>;
}

impl<T: TransportProtocol> Reconnect for Arc<SessionCreator<T>> {
    fn connect(
        &self,
        url: String,
        generation: u64,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> BoxFuture<anyhow::Result<(Connection, ControlReceiver)>> {
        let creator = self.clone();
        Box::pin(async move {
            let transport_conn = creator.open_transport(&url).await?;
            let (connection, control_receiver, _) = creator
                .set_up(transport_conn, generation, event_sender)
                .await?;
            Ok((connection, control_receiver))
        })
    }
}

impl<T: TransportProtocol> SessionCreator<T> {
    pub(crate) async fn create_new_connection(
        self: &Arc<Self>,
        url: &str,
    ) -> anyhow::Result<Connecting> {
        let transport_conn = self.open_transport(url).await?;
        let creator = self.clone();
        let url = url.to_string();
        let handshake = async move {
            let (event_sender, event_receiver) = tokio::sync::mpsc::unbounded_channel();
            let session = if creator.follow_go_away {
                let (connection_event_sender, connection_events) =
                    tokio::sync::mpsc::unbounded_channel();
                let (connection, control_receiver, server_setup) = creator
                    .set_up(transport_conn, 0, connection_event_sender)
                    .await?;
                Session::new(
                    control_receiver,
                    SessionContext::new(connection, event_sender, true),
                    event_receiver,
                    Some(server_setup),
                    Some(Follow {
                        reconnect: Box::new(creator.clone()),
                        url,
                        connection_events,
                    }),
                )
            } else {
                let (connection, control_receiver, server_setup) = creator
                    .set_up(transport_conn, 0, event_sender.clone())
                    .await?;
                Session::new(
                    control_receiver,
                    SessionContext::new(connection, event_sender, false),
                    event_receiver,
                    Some(server_setup),
                    None,
                )
            };
            tracing::info!("Session established");
            Ok(session)
        };
        Ok(Connecting {
            inner: Box::pin(handshake),
        })
    }

    async fn open_transport(&self, url: &str) -> anyhow::Result<BoxedConnection> {
        let target = ConnectTarget::parse(url)?;
        self.transport_creator.create_new_transport(&target).await
    }

    /// The CLIENT_SETUP / SERVER_SETUP exchange on a new transport.
    async fn set_up(
        &self,
        transport_conn: BoxedConnection,
        generation: u64,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> anyhow::Result<(Connection, ControlReceiver, ServerSetup)> {
        let (control, control_receiver, server_setup) =
            match version::connect(&transport_conn, self.authorization_token.as_deref()).await {
                Ok(established) => established,
                Err(error) => {
                    return Err(Self::explain_rejected_setup(&transport_conn, error).await);
                }
            };
        Ok((
            Connection::new(transport_conn, control, false, generation, event_sender),
            control_receiver,
            server_setup,
        ))
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

    pub(crate) async fn accept_new_connection(&self) -> Result<Accepting, AcceptError> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let handshake = async move {
            let setup = version::accept(&transport_conn).await?;
            Ok(Handshake {
                transport_connection: transport_conn,
                setup,
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
                follow_go_away: false,
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
            handshake.authorization_tokens(),
            [AuthorizationToken::use_value_utf8("jwt")]
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
        assert!(handshake.authorization_tokens().is_empty());
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
            client_context.connection().control.next_request_id(),
            client_context.connection().control.next_request_id(),
        ];
        let server_ids = [
            server_context.connection().control.next_request_id(),
            server_context.connection().control.next_request_id(),
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
