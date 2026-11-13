use std::{pin::pin, sync::Arc};

use anyhow::Context;
use tracing::{Instrument, Span};

use crate::TerminationErrorCode;
use crate::modules::executor::{self, BoxFuture, MaybeSend, MaybeSync};
use crate::modules::moqt::domains::connection::Connection;
use crate::modules::moqt::domains::endpoint::AuthorizationTokenSource;
use crate::modules::moqt::domains::session::{Follow, Session};
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::domains::session_events::{self, SessionEventSender};
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::version::{self, ControlReceiver, Version};
use crate::modules::transport::accept_error::AcceptError;
use crate::modules::transport::connect_target::ConnectTarget;
use crate::modules::transport::transport_connection::BoxedConnection;
use crate::modules::transport::transport_connection::TransportClose;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use std::time::Duration;

use crate::{Connecting, server::Accepting, server::Handshake};

const CLOSE_REASON_TIMEOUT: Duration = Duration::from_secs(1);
/// QUIC's idle timeout does not end the handshake of a peer that answers
/// keep-alives but withholds the WebTransport CONNECT.
const TRANSPORT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Nor does it end a connection whose peer never sends CLIENT_SETUP. The wait
/// includes the client's token provider, which runs once the transport is open.
const CLIENT_SETUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
#[error("peer closed the connection before SERVER_SETUP (code {code}: {reason})")]
pub(crate) struct SetupRejected {
    pub(crate) code: u32,
    reason: String,
}

/// One connection attempt, from opening the transport to SERVER_SETUP. Not a
/// child of the caller's span, which may live as long as the session (a
/// relay's inter-relay session span) and would hold it unexported.
fn connect_span(url: &str) -> Span {
    let span = tracing::info_span!(
        parent: None,
        "moqt.session.connect",
        url = %url,
        outcome = tracing::field::Empty,
    );
    span.follows_from(Span::current());
    span
}

pub(crate) struct SessionCreator<T: TransportProtocol> {
    pub(crate) transport_creator: T::ConnectionCreator,
    pub(crate) authorization_token: Option<AuthorizationTokenSource>,
    pub(crate) follow_go_away: bool,
    /// `T::ConnectionCreator::WEB_SOCKET_HEAD_START`, which tests shorten.
    pub(super) web_socket_head_start: Option<Duration>,
    /// The drafts the endpoint offers (a client) or accepts (a server).
    pub(crate) versions: Vec<Version>,
}

/// Opens another connection of a session that follows GOAWAY, on the same
/// endpoint and transport as the first.
pub(crate) trait Reconnect: MaybeSend + MaybeSync {
    fn refreshes_token(&self) -> bool;

    fn connect(
        &self,
        url: String,
        generation: u64,
        event_sender: SessionEventSender,
    ) -> BoxFuture<anyhow::Result<(Connection, ControlReceiver)>>;
}

/// The endpoint of a client session, whose transport `Session::close` waits
/// to finish closing.
pub(crate) trait WaitIdle: MaybeSend + MaybeSync {
    fn wait_idle(&self) -> BoxFuture<()>;
}

impl<T: TransportProtocol> WaitIdle for Arc<SessionCreator<T>> {
    fn wait_idle(&self) -> BoxFuture<()> {
        let creator = self.clone();
        Box::pin(async move { creator.transport_creator.wait_idle().await })
    }
}

impl<T: TransportProtocol> Reconnect for Arc<SessionCreator<T>> {
    fn refreshes_token(&self) -> bool {
        self.authorization_token
            .as_ref()
            .is_some_and(AuthorizationTokenSource::is_provider)
    }

    fn connect(
        &self,
        url: String,
        generation: u64,
        event_sender: SessionEventSender,
    ) -> BoxFuture<anyhow::Result<(Connection, ControlReceiver)>> {
        let creator = self.clone();
        Box::pin(async move {
            let target = ConnectTarget::parse(&url)?;
            let connect_span = connect_span(&url);
            let transport_conn = creator.open_transport_for(&target, &connect_span).await?;
            let (connection, control_receiver) = creator
                .set_up_in(
                    connect_span,
                    transport_conn,
                    &target,
                    generation,
                    event_sender,
                )
                .await?;
            Ok((connection, control_receiver))
        })
    }
}

impl<T: TransportProtocol> SessionCreator<T> {
    pub(crate) fn new(
        transport_creator: T::ConnectionCreator,
        authorization_token: Option<AuthorizationTokenSource>,
        follow_go_away: bool,
        versions: Vec<Version>,
    ) -> Self {
        Self {
            transport_creator,
            authorization_token,
            follow_go_away,
            web_socket_head_start: T::ConnectionCreator::WEB_SOCKET_HEAD_START,
            versions,
        }
    }

    pub(crate) async fn create_new_connection(
        self: &Arc<Self>,
        url: &str,
    ) -> anyhow::Result<Connecting> {
        let target = ConnectTarget::parse(url)?;
        let connect_span = connect_span(url);
        let transport_conn = self.open_transport_for(&target, &connect_span).await?;
        let creator = self.clone();
        let url = url.to_string();
        let handshake = async move {
            let (event_sender, event_receiver) = session_events::channel();
            let (connection_event_sender, follow) = if creator.follow_go_away {
                let (connection_event_sender, connection_events) = session_events::channel();
                let follow = Follow {
                    reconnect: Arc::new(creator.clone()),
                    url,
                    connection_events,
                };
                (connection_event_sender, Some(follow))
            } else {
                (event_sender.clone(), None)
            };
            let (connection, control_receiver) = creator
                .set_up_in(
                    connect_span,
                    transport_conn,
                    &target,
                    0,
                    connection_event_sender,
                )
                .await?;
            let session = Session::new(
                control_receiver,
                SessionContext::new(connection, event_sender, creator.follow_go_away),
                event_receiver,
                follow,
                Some(Arc::new(creator.clone())),
                Span::current(),
            );
            tracing::info!("session established");
            Ok(session)
        };
        Ok(Connecting {
            inner: Box::pin(handshake),
        })
    }

    /// Opens the transport outside `connect_span`: quinn runs a connection's
    /// driver in the span current when it connects, which would hold the
    /// connect span open for as long as the connection lives. The
    /// "transport connected" event uses `in_scope` rather than `parent:`
    /// because the browser's telemetry attaches a log to the current span.
    async fn open_transport_for(
        &self,
        target: &ConnectTarget,
        connect_span: &Span,
    ) -> anyhow::Result<BoxedConnection> {
        let opened = self.open_transport(target).await;
        match &opened {
            Ok(connection) => connect_span.in_scope(|| {
                tracing::info!(
                    transport = connection.negotiated_protocol().transport_name(),
                    remote_address = ?connection.addresses().remote,
                    "transport connected"
                )
            }),
            Err(_) => {
                connect_span.record("outcome", "failed");
            }
        }
        opened
    }

    /// Closes `connect_span` before the caller starts the session, whose tasks
    /// would otherwise hold it.
    async fn set_up_in(
        &self,
        connect_span: Span,
        transport_conn: BoxedConnection,
        target: &ConnectTarget,
        generation: u64,
        event_sender: SessionEventSender,
    ) -> anyhow::Result<(Connection, ControlReceiver)> {
        let set_up = self
            .set_up(transport_conn, target, generation, event_sender)
            .instrument(connect_span.clone())
            .await;
        let outcome = match &set_up {
            Ok(_) => "ok",
            Err(error) if error.downcast_ref::<SetupRejected>().is_some() => "rejected",
            Err(_) => "failed",
        };
        connect_span.record("outcome", outcome);
        set_up
    }

    async fn open_transport(&self, target: &ConnectTarget) -> anyhow::Result<BoxedConnection> {
        let Some(head_start) = self.web_socket_head_start else {
            return self.transport_creator.create_new_transport(target).await;
        };
        if target.web_socket_fallback().is_none() {
            return self.transport_creator.create_new_transport(target).await;
        }
        let target = target.clone().resolved().await?;
        let web_socket = target
            .web_socket_fallback()
            .expect("a target that falls back still does once resolved");
        self.race(&target, &web_socket, head_start).await
    }

    /// Happy Eyeballs (RFC 8305) between `target` and `web_socket`. Dropping the
    /// losing dial cancels it.
    async fn race(
        &self,
        target: &ConnectTarget,
        web_socket: &ConnectTarget,
        head_start: Duration,
    ) -> anyhow::Result<BoxedConnection> {
        let mut primary = pin!(self.transport_creator.create_new_transport(target));
        let early_failure = match executor::timeout(head_start, primary.as_mut()).await {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => Some(error),
            Err(()) => None,
        };
        let mut fallback = pin!(self.transport_creator.create_new_transport(web_socket));
        let primary_error = match early_failure {
            Some(error) => error,
            None => tokio::select! {
                biased;
                result = primary.as_mut() => match result {
                    Ok(connection) => return Ok(connection),
                    Err(error) => error,
                },
                result = fallback.as_mut() => match result {
                    Ok(connection) => {
                        tracing::warn!(
                            url = %target.url,
                            fallback_url = %web_socket.url,
                            "WebSocket won the transport race"
                        );
                        return Ok(connection);
                    }
                    Err(web_socket_error) => {
                        return primary.await.map_err(|error| {
                            Self::both_failed(target, &error, web_socket, &web_socket_error)
                        });
                    }
                },
            },
        };
        match fallback.await {
            Ok(connection) => {
                tracing::warn!(
                    url = %target.url,
                    fallback_url = %web_socket.url,
                    error = %format!("{primary_error:#}"),
                    "falling back to WebSocket"
                );
                Ok(connection)
            }
            Err(web_socket_error) => Err(Self::both_failed(
                target,
                &primary_error,
                web_socket,
                &web_socket_error,
            )),
        }
    }

    fn both_failed(
        target: &ConnectTarget,
        error: &anyhow::Error,
        web_socket: &ConnectTarget,
        web_socket_error: &anyhow::Error,
    ) -> anyhow::Error {
        anyhow::anyhow!(
            "{} failed: {error:#}; {} failed: {web_socket_error:#}",
            target.url,
            web_socket.url
        )
    }

    /// The CLIENT_SETUP / SERVER_SETUP exchange on a new transport.
    async fn set_up(
        &self,
        transport_conn: BoxedConnection,
        target: &ConnectTarget,
        generation: u64,
        event_sender: SessionEventSender,
    ) -> anyhow::Result<(Connection, ControlReceiver)> {
        let token = match &self.authorization_token {
            Some(source) => Some(
                source
                    .token()
                    .await
                    .context("the authorization token provider failed")?,
            ),
            None => None,
        };
        let (control, control_receiver) =
            match version::connect(&transport_conn, &self.versions, token.as_deref(), target).await
            {
                Ok(established) => established,
                Err(error) => {
                    return Err(Self::explain_rejected_setup(&transport_conn, error).await);
                }
            };
        Ok((
            Connection::new(transport_conn, control, false, generation, event_sender),
            control_receiver,
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
            }) => error.context(SetupRejected { code, reason }),
            _ => error,
        }
    }

    pub(crate) async fn accept_new_connection(&self) -> Result<Accepting, AcceptError> {
        let transport_accepting = self.transport_creator.accept_new_transport().await?;
        let versions = self.versions.clone();
        let handshake = async move {
            let transport_conn =
                executor::timeout(TRANSPORT_HANDSHAKE_TIMEOUT, transport_accepting)
                    .await
                    .map_err(|()| anyhow::anyhow!("the transport handshake timed out"))??;
            let setup = accept_setup(&transport_conn, &versions).await?;
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

async fn accept_setup(
    transport_conn: &BoxedConnection,
    accepted: &[Version],
) -> anyhow::Result<version::ServerHandshake> {
    if let Ok(setup) = executor::timeout(
        CLIENT_SETUP_TIMEOUT,
        version::accept(transport_conn, accepted),
    )
    .await
    {
        return setup;
    }
    let code = version::negotiated_version(transport_conn, accepted)?
        .termination_code(TerminationErrorCode::ControlMessageTimeout);
    transport_conn.close(code, "CLIENT_SETUP timed out");
    // WebTransport sends the code only while the connection outlives the
    // close, as in `Handshake::reject`.
    let _closed = executor::timeout(CLOSE_REASON_TIMEOUT, transport_conn.closed()).await;
    anyhow::bail!("the peer sent no CLIENT_SETUP within {CLIENT_SETUP_TIMEOUT:?}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        AuthorizationTokenSource, CertificateVerification, ClientConfig, Endpoint, NATIVE, Session,
        TerminationErrorCode,
        modules::{
            moqt::{
                control_plane::{
                    constants::MOQ_TRANSPORT_VERSION,
                    control_messages::messages::parameters::authorization_token::AuthorizationToken,
                },
                version::application_protocols,
            },
            test_support::{
                DRAFTS, HANDSHAKE_TIMEOUT, TEST_VERSIONS, WEB_SOCKET_ONLY, capture_telemetry,
                connected_sessions, native_client, native_client_with_config,
                self_signed_server_config, slow_udp_path, spawn_connected_native_sessions,
                spawn_native_server, spawn_native_server_handshake, udp_blocked_server, within,
            },
            transport::client_crypto::{client_crypto, client_endpoint, quic_client_config},
        },
    };

    async fn silent_quic_peer(port: u16, alpn: &str) -> quinn::Connection {
        let endpoint = client_endpoint(0).unwrap();
        let config = quic_client_config(
            client_crypto(&CertificateVerification::Disabled).unwrap(),
            vec![alpn.as_bytes().to_vec()],
        )
        .unwrap();
        let address = (std::net::Ipv4Addr::LOCALHOST, port).into();
        endpoint
            .connect_with(config, address, "localhost")
            .unwrap()
            .await
            .unwrap()
    }

    fn falling_back_client() -> Endpoint<NATIVE> {
        native_client_with_config(ClientConfig {
            certificate_verification: CertificateVerification::Disabled,
            follow_go_away: false,
            ..ClientConfig::default()
        })
    }

    /// A server that turns QUIC away and answers WebSocket with 503, both at once.
    fn refusing_server(name: &str) -> u16 {
        let server = Endpoint::<NATIVE>::create_server(&self_signed_server_config(name)).unwrap();
        let port = server.local_port().unwrap();
        server.refuse_new_connections();
        tokio::spawn(async move { while server.accept().await.is_ok() {} });
        port
    }

    /// Accepts sessions on `server` until the test ends, keeping them open.
    fn accept_sessions(
        server: Endpoint<WEB_SOCKET_ONLY>,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Session> {
        let (session_sender, session_receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(accepting) = server.accept().await {
                let session = accepting
                    .await
                    .unwrap()
                    .accept(&tracing::Span::current())
                    .await
                    .unwrap();
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
        let client = falling_back_client().with_web_socket_head_start(Duration::from_millis(100));

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
        let client = falling_back_client().with_web_socket_head_start(Duration::from_millis(100));

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
    async fn quic_that_opens_after_its_head_start_still_beats_a_web_socket_that_does_not() {
        // Arrange
        let (server_port, accept) = spawn_native_server("race-quic-wins");
        let path = slow_udp_path(server_port, Duration::from_millis(300)).await;
        let client = falling_back_client().with_web_socket_head_start(Duration::from_millis(50));

        // Act
        let session = within(async {
            client
                .connect(&format!("moqt://127.0.0.1:{}", path.port))
                .await?
                .await
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(session.url_scheme(), "moqt");
        within(accept).await.unwrap();
    }

    #[tokio::test]
    async fn both_failures_are_reported_when_web_socket_fails_too() {
        // Arrange
        let port = refusing_server("fallback-both-fail");
        let client = falling_back_client();

        // Act
        let error = within(client.connect(&format!("moqt://127.0.0.1:{port}")))
            .await
            .err()
            .expect("the server refuses both");

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
        let (port, accept) = spawn_native_server_handshake("client-setup-with-token");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move {
            native_client_with_config(ClientConfig {
                certificate_verification: CertificateVerification::Disabled,
                authorization_token: Some("jwt".to_string().into()),
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
    async fn connect_sends_the_token_the_provider_returns() {
        // Arrange
        let (port, accept) = spawn_native_server_handshake("client-setup-token-provider");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move {
            native_client_with_config(ClientConfig {
                authorization_token: Some(AuthorizationTokenSource::provider(|| async {
                    Ok("fresh-jwt".to_string())
                })),
                certificate_verification: CertificateVerification::Disabled,
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
            [AuthorizationToken::use_value_utf8("fresh-jwt")]
        );
        client.abort();
    }

    #[tokio::test]
    async fn a_failing_token_provider_fails_the_connect() {
        // Arrange
        let (port, _accept) = spawn_native_server_handshake("client-setup-token-provider-error");
        let url = format!("moqt://127.0.0.1:{port}");
        let endpoint = native_client_with_config(ClientConfig {
            authorization_token: Some(AuthorizationTokenSource::provider(|| async {
                anyhow::bail!("token backend unreachable")
            })),
            certificate_verification: CertificateVerification::Disabled,
            ..ClientConfig::default()
        });

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            endpoint.connect(&url).await?.await
        })
        .await
        .unwrap();

        // Assert
        let Err(error) = result else {
            panic!("expected the connect to fail");
        };
        assert!(
            format!("{error:#}").contains("token backend unreachable"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn connect_without_token_sends_no_authorization_token() {
        // Arrange
        let (port, accept) = spawn_native_server_handshake("client-setup-without-token");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { native_client().connect(&url).await?.await });

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
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("request-id-parity", version).await;
            let client_context = client.publisher().session;
            let server_context = server.publisher().session;

            // Act
            let client_ids = [
                client_context.connection().control.take_request_id().await,
                client_context.connection().control.take_request_id().await,
            ];
            let server_ids = [
                server_context.connection().control.take_request_id().await,
                server_context.connection().control.take_request_id().await,
            ];

            // Assert
            assert_eq!(client_ids, [0, 2]);
            assert_eq!(server_ids, [1, 3]);
        }
    }

    async fn connect_to_a_rejecting_server(name: &str) -> anyhow::Error {
        let (port, accept) = spawn_native_server_handshake(name);
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { native_client().connect(&url).await?.await });
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();
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
        error
    }

    #[tokio::test]
    async fn a_rejected_client_setup_reports_the_peer_close_code_and_reason() {
        // Act
        let error = connect_to_a_rejecting_server("session-creator-rejected-setup").await;

        // Assert
        assert!(
            format!("{error:#}").contains("code 2: bad token"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn the_connect_span_closes_with_ok_while_the_session_lives() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();

        // Act
        let (_client, _server) =
            spawn_connected_native_sessions("session-creator-connect-span").await;

        // Assert
        let spans = telemetry.closed_spans("moqt.session.connect");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].field("outcome"), Some("ok"));
    }

    #[tokio::test]
    async fn the_connect_span_records_when_the_transport_opened_and_client_setup_was_sent() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();

        // Act
        let (_client, _server) =
            spawn_connected_native_sessions("session-creator-connect-events").await;

        // Assert
        let connected = telemetry.events("transport connected");
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].span(), Some("moqt.session.connect"));
        assert_eq!(connected[0].field("transport"), Some("QUIC"));
        let client_setup = telemetry.events("CLIENT_SETUP sent");
        assert_eq!(client_setup[0].span(), Some("moqt.session.connect"));
    }

    #[tokio::test]
    async fn each_side_logs_the_setup_it_sends_and_receives() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let version = MOQ_TRANSPORT_VERSION.to_string();

        // Act
        let (_client, _server) =
            spawn_connected_native_sessions("session-creator-setup-logs").await;

        // Assert
        for (message, field) in [
            ("CLIENT_SETUP sent", "supported_versions"),
            ("CLIENT_SETUP received", "supported_versions"),
            ("SERVER_SETUP sent", "selected_version"),
            ("SERVER_SETUP received", "selected_version"),
        ] {
            let logged = telemetry.events(message);
            assert_eq!(logged.len(), 1, "{message}");
            assert!(
                logged[0]
                    .field(field)
                    .is_some_and(|value| value.contains(&version)),
                "{message}"
            );
        }
    }

    #[tokio::test]
    async fn a_rejected_client_setup_ends_the_connect_span_as_rejected() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();

        // Act
        connect_to_a_rejecting_server("session-creator-rejected-connect-span").await;

        // Assert
        let spans = telemetry.closed_spans("moqt.session.connect");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].field("outcome"), Some("rejected"));
    }

    #[tokio::test]
    async fn a_peer_stalling_its_web_transport_handshake_does_not_hold_back_the_next_connection() {
        // Arrange: the server spawns every `Accepting`, as the relay does
        let server =
            Endpoint::<NATIVE>::create_server(&self_signed_server_config("accept-stalled-peer"))
                .unwrap();
        let port = server.local_port().unwrap();
        let (session_sender, mut session_receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(accepting) = server.accept().await {
                let session_sender = session_sender.clone();
                tokio::spawn(async move {
                    let Ok(handshake) = accepting.await else {
                        return;
                    };
                    let session = handshake.accept(&tracing::Span::current()).await;
                    let _ = session_sender.send(session);
                });
            }
        });
        let _stalled = within(silent_quic_peer(port, web_transport_quinn::ALPN)).await;

        // Act
        let session = within(async {
            native_client()
                .connect(&format!("moqt://127.0.0.1:{port}"))
                .await?
                .await
        })
        .await;

        // Assert
        session.unwrap();
        within(session_receiver.recv()).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_peer_that_sends_no_client_setup_is_closed_after_the_deadline() {
        // Arrange: the server waits for the control stream
        let (telemetry, _guard) = capture_telemetry();
        let server =
            Endpoint::<NATIVE>::create_server(&self_signed_server_config("accept-silent-peer"))
                .unwrap();
        let port = server.local_port().unwrap();
        let accepting = tokio::spawn(async move { server.accept().await.unwrap().await.err() });
        let silent = within(silent_quic_peer(
            port,
            application_protocols(TEST_VERSIONS).unwrap().quic_alpns[0],
        ))
        .await;
        within(async {
            while telemetry.events("MoQT version negotiated").is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        tokio::time::pause();

        // Act
        let error =
            tokio::time::timeout(super::CLIENT_SETUP_TIMEOUT + HANDSHAKE_TIMEOUT, accepting)
                .await
                .expect("the server still waits for CLIENT_SETUP")
                .unwrap();
        let closed = silent.closed().await;

        // Assert: the peer sees CONTROL_MESSAGE_TIMEOUT (0x11)
        let error = error.expect("a peer without CLIENT_SETUP was accepted");
        assert!(
            format!("{error:#}").contains("no CLIENT_SETUP"),
            "{error:#}"
        );
        let quinn::ConnectionError::ApplicationClosed(close) = closed else {
            panic!("expected an application close, got {closed:?}");
        };
        assert_eq!(close.error_code, quinn::VarInt::from_u32(0x11));
    }
}
