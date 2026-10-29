use std::{collections::HashSet, sync::Arc};

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

use crate::{Connecting, WebSocketFallback, server::Accepting, server::Handshake};

const CLOSE_REASON_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
    pub(crate) authorization_token: Option<String>,
    pub(crate) follow_go_away: bool,
    web_socket_fallback: WebSocketFallback,
    /// The hosts and ports that fell back to WebSocket; never tried with
    /// QUIC or WebTransport again.
    fell_back: std::sync::Mutex<HashSet<String>>,
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
    pub(crate) fn new(
        transport_creator: T::ConnectionCreator,
        authorization_token: Option<String>,
        follow_go_away: bool,
        web_socket_fallback: WebSocketFallback,
    ) -> Self {
        Self {
            transport_creator,
            authorization_token,
            follow_go_away,
            web_socket_fallback,
            fell_back: std::sync::Mutex::new(HashSet::new()),
        }
    }

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
            tracing::info!("session established");
            Ok(session)
        };
        Ok(Connecting {
            inner: Box::pin(handshake),
        })
    }

    async fn open_transport(&self, url: &str) -> anyhow::Result<BoxedConnection> {
        let target = ConnectTarget::parse(url)?;
        let wait = match self.web_socket_fallback {
            WebSocketFallback::After(wait) if T::ConnectionCreator::FALLS_BACK_TO_WEB_SOCKET => {
                wait
            }
            _ => return self.transport_creator.create_new_transport(&target).await,
        };
        let Some(web_socket) = target.web_socket_fallback() else {
            return self.transport_creator.create_new_transport(&target).await;
        };
        let authority = target.authority();
        if self
            .fell_back
            .lock()
            .expect("fallback lock")
            .contains(&authority)
        {
            return self
                .transport_creator
                .create_new_transport(&web_socket)
                .await;
        }
        let target = target.resolved().await?;
        let web_socket = target
            .web_socket_fallback()
            .expect("a target that falls back still does once resolved");
        let error =
            match executor::timeout(wait, self.transport_creator.create_new_transport(&target))
                .await
            {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(error)) => error,
                Err(()) => anyhow::anyhow!("no answer within {wait:?}"),
            };
        tracing::warn!(
            %url,
            fallback_url = %web_socket.url,
            error = %format!("{error:#}"),
            "falling back to WebSocket"
        );
        match self
            .transport_creator
            .create_new_transport(&web_socket)
            .await
        {
            Ok(connection) => {
                self.fell_back
                    .lock()
                    .expect("fallback lock")
                    .insert(authority);
                Ok(connection)
            }
            Err(web_socket_error) => Err(anyhow::anyhow!(
                "{url} failed: {error:#}; {} failed: {web_socket_error:#}",
                web_socket.url
            )),
        }
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
    use std::time::Duration;

    use crate::{
        ClientConfig, DUAL, Endpoint, Session, TerminationErrorCode, WebSocketFallback,
        modules::{
            moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken,
            test_support::{
                HANDSHAKE_TIMEOUT, WEB_SOCKET_ONLY, dual_client, dual_client_with_config,
                self_signed_server_config, spawn_connected_dual_sessions,
                spawn_dual_server_handshake, udp_blocked_server, unreachable_port, within,
            },
        },
    };

    fn falling_back_client(fallback: WebSocketFallback) -> Endpoint<DUAL> {
        dual_client_with_config(ClientConfig {
            verify_certificate: false,
            follow_go_away: false,
            web_socket_fallback: fallback,
            ..ClientConfig::default()
        })
    }

    /// Accepts sessions on `server` until the test ends, keeping them open.
    fn accept_sessions(
        server: Endpoint<WEB_SOCKET_ONLY>,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Session> {
        let (session_sender, session_receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(accepting) = server.accept().await {
                let session = accepting.await.unwrap().accept().await.unwrap();
                if session_sender.send(session).is_err() {
                    return;
                }
            }
        });
        session_receiver
    }

    #[tokio::test]
    async fn a_moqt_url_falls_back_to_web_socket_when_quic_gets_no_answer() {
        // Arrange
        let blocked = udp_blocked_server("fallback-moqt");
        let mut accepted = accept_sessions(blocked.server);
        let client = falling_back_client(WebSocketFallback::After(Duration::from_millis(300)));

        // Act
        let session = within(async {
            client
                .connect(&format!("moqt://127.0.0.1:{}", blocked.port))
                .await?
                .await
        })
        .await
        .unwrap();
        let server_session = within(accepted.recv()).await.unwrap();

        // Assert
        assert_eq!(session.url_scheme(), "wss");
        assert_eq!(server_session.url_scheme(), "wss");
    }

    #[tokio::test]
    async fn an_https_url_falls_back_to_web_socket_when_web_transport_gets_no_answer() {
        // Arrange
        let blocked = udp_blocked_server("fallback-https");
        let mut accepted = accept_sessions(blocked.server);
        let client = falling_back_client(WebSocketFallback::After(Duration::from_millis(300)));

        // Act
        let session = within(async {
            client
                .connect(&format!("https://127.0.0.1:{}/moq", blocked.port))
                .await?
                .await
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(session.url_scheme(), "wss");
        within(accepted.recv()).await.unwrap();
    }

    #[tokio::test]
    async fn a_host_that_fell_back_is_opened_over_web_socket_at_once() {
        // Arrange: the first connection waits out the whole fallback delay
        let wait = Duration::from_secs(1);
        let blocked = udp_blocked_server("fallback-remembered");
        let mut accepted = accept_sessions(blocked.server);
        let client = falling_back_client(WebSocketFallback::After(wait));
        let url = format!("moqt://127.0.0.1:{}", blocked.port);
        let _first = tokio::time::timeout(2 * wait + HANDSHAKE_TIMEOUT, async {
            client.connect(&url).await?.await
        })
        .await
        .unwrap()
        .unwrap();

        // Act
        let started = std::time::Instant::now();
        let second = within(async { client.connect(&url).await?.await })
            .await
            .unwrap();
        let elapsed = started.elapsed();

        // Assert
        assert!(elapsed < wait, "the second connection waited {elapsed:?}");
        assert_eq!(second.url_scheme(), "wss");
        within(accepted.recv()).await.unwrap();
    }

    #[tokio::test]
    async fn a_disabled_fallback_returns_the_quic_failure_alone() {
        // Arrange: a refusing server turns QUIC away at once
        let server =
            Endpoint::<DUAL>::create_server(&self_signed_server_config("fallback-disabled"))
                .unwrap();
        let port = server.local_port().unwrap();
        server.refuse_new_connections();
        let _accept = tokio::spawn(async move { while server.accept().await.is_ok() {} });
        let client = falling_back_client(WebSocketFallback::Disabled);

        // Act
        let error = within(client.connect(&format!("moqt://127.0.0.1:{port}")))
            .await
            .err()
            .expect("the server refuses");

        // Assert
        let message = format!("{error:#}");
        assert!(!message.contains("wss://"), "{message}");
    }

    #[tokio::test]
    async fn both_failures_are_reported_when_web_socket_fails_too() {
        // Arrange
        let (port, _udp) = unreachable_port();
        let client = falling_back_client(WebSocketFallback::After(Duration::from_millis(200)));

        // Act
        let error = within(client.connect(&format!("moqt://127.0.0.1:{port}")))
            .await
            .err()
            .expect("nothing listens on the port");

        // Assert
        let message = format!("{error:#}");
        assert!(
            message.contains(&format!("moqt://127.0.0.1:{port} failed")),
            "{message}"
        );
        assert!(
            message.contains(&format!("wss://127.0.0.1:{port}/ failed")),
            "{message}"
        );
    }

    #[tokio::test]
    async fn connect_sends_authorization_token_in_client_setup() {
        // Arrange
        let (port, accept) = spawn_dual_server_handshake("client-setup-with-token");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move {
            dual_client_with_config(ClientConfig {
                verify_certificate: false,
                authorization_token: Some("jwt".to_string()),
                follow_go_away: false,
                ..ClientConfig::default()
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
        let client_id = client_context
            .connection()
            .control
            .reserve_request_id()
            .await
            .request_id();
        let server_id = server_context
            .connection()
            .control
            .reserve_request_id()
            .await
            .request_id();

        // Assert
        assert_eq!(client_id, 0);
        assert_eq!(server_id, 1);
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
