use std::{path::PathBuf, sync::Arc, time::Duration};

use crate::{
    Connecting, TransportProtocol,
    executor::{MaybeSend, MaybeSync},
    modules::{
        executor::BoxFuture,
        moqt::{domains::session_creator::SessionCreator, version::APPLICATION_PROTOCOLS},
        transport::{
            accept_error::AcceptError, transport_connection_creator::TransportConnectionCreator,
        },
    },
    server::Accepting,
};

/// The AUTHORIZATION TOKEN a client sends in CLIENT_SETUP (draft-14 §9.2.1.1).
#[derive(Clone)]
pub struct AuthorizationTokenSource(TokenSource);

#[derive(Clone)]
enum TokenSource {
    Fixed(String),
    Provider(Arc<dyn TokenProvider>),
}

trait TokenProvider: MaybeSend + MaybeSync {
    fn token(&self) -> BoxFuture<anyhow::Result<String>>;
}

impl<F, Fut> TokenProvider for F
where
    F: Fn() -> Fut + MaybeSend + MaybeSync,
    Fut: Future<Output = anyhow::Result<String>> + MaybeSend + 'static,
{
    fn token(&self) -> BoxFuture<anyhow::Result<String>> {
        Box::pin(self())
    }
}

impl AuthorizationTokenSource {
    /// `provider` is called before every CLIENT_SETUP, including on connections
    /// opened to follow GOAWAY. An `Err` fails that connection.
    pub fn provider<F, Fut>(provider: F) -> Self
    where
        F: Fn() -> Fut + MaybeSend + MaybeSync + 'static,
        Fut: Future<Output = anyhow::Result<String>> + MaybeSend + 'static,
    {
        Self(TokenSource::Provider(Arc::new(provider)))
    }

    pub(crate) fn is_provider(&self) -> bool {
        matches!(self.0, TokenSource::Provider(_))
    }

    pub(crate) async fn token(&self) -> anyhow::Result<String> {
        match &self.0 {
            TokenSource::Fixed(token) => Ok(token.clone()),
            TokenSource::Provider(provider) => provider.token().await,
        }
    }
}

impl From<String> for AuthorizationTokenSource {
    fn from(token: String) -> Self {
        Self(TokenSource::Fixed(token))
    }
}

/// `Endpoint::create_client` fails on `BROWSER` for every variant but `NativeRoots`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CertificateVerification {
    /// Trusts the operating system's root certificates.
    #[default]
    NativeRoots,
    /// Trusts only the certificates of this PEM file.
    TrustedCertificates(PathBuf),
    Disabled,
}

impl CertificateVerification {
    pub fn native_roots_or_disabled(verify: bool) -> Self {
        if verify {
            Self::NativeRoots
        } else {
            Self::Disabled
        }
    }
}

pub struct ClientConfig {
    pub port: u16,
    pub certificate_verification: CertificateVerification,
    pub authorization_token: Option<AuthorizationTokenSource>,
    /// A session follows the server's GOAWAY by itself (draft-14 §3.5):
    /// it connects to the New Session URI, sends its subscriptions there
    /// again and keeps the request ids and track aliases the application
    /// knows. Such a session also reconnects to its URL, the same way, when its
    /// connection ends without a termination code, or when the server closes it
    /// for an expired token and `authorization_token` is a provider. With `false`,
    /// GOAWAY arrives as a `SessionEvent` and the session stays on its connection.
    pub follow_go_away: bool,
    pub web_socket_fallback: WebSocketFallback,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: 0,
            certificate_verification: CertificateVerification::NativeRoots,
            authorization_token: None,
            follow_go_away: true,
            web_socket_fallback: WebSocketFallback::default(),
        }
    }
}

/// Whether a `NATIVE` or `BROWSER` client also dials QMux over WebSocket
/// (`wss://`, same host and port) for a `moqt://` or `https://` URL; a browser
/// does so for `https://` only. QUIC or WebTransport gets a head start, then
/// WebSocket is dialed alongside it and the first to open is used; a failure
/// within the head start dials WebSocket at once. Only opening the transport
/// races; a failed SETUP does not fall back. `QUIC` and `WEBTRANSPORT` ignore
/// this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebSocketFallback {
    Disabled,
    /// The endpoint's own head start: 200 ms for `NATIVE`, 500 ms for `BROWSER`.
    #[default]
    Enabled,
    HeadStart(Duration),
}

#[derive(Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    // log_level: String,
}

pub struct Endpoint<T: TransportProtocol> {
    session_creator: Arc<SessionCreator<T>>,
}

impl<T: TransportProtocol> Endpoint<T> {
    pub fn create_client(config: &ClientConfig) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client(
            config.port,
            &config.certificate_verification,
            APPLICATION_PROTOCOLS,
        )?;
        let session_creator = SessionCreator::new(
            client,
            config.authorization_token.clone(),
            config.follow_go_away,
            config.web_socket_fallback,
        );
        Ok(Self {
            session_creator: Arc::new(session_creator),
        })
    }

    pub fn create_server(server_config: &ServerConfig) -> anyhow::Result<Self> {
        let server = T::ConnectionCreator::server(
            &server_config.cert_path,
            &server_config.key_path,
            server_config.port,
            server_config.keep_alive_interval_sec,
            APPLICATION_PROTOCOLS,
        )?;
        let session_creator = SessionCreator::new(server, None, false, WebSocketFallback::Disabled);
        Ok(Self {
            session_creator: Arc::new(session_creator),
        })
    }

    /// `url` selects the transport by scheme: `moqt://host[:port]` for raw
    /// QUIC (default port 4433), `https://host[:port][/path]` for
    /// WebTransport and `wss://host[:port][/path]` for QMux over WebSocket
    /// (both default port 443). `QUIC` and `WEBTRANSPORT` endpoints accept
    /// only their own scheme; `NATIVE` accepts all three.
    pub async fn connect(&self, url: &str) -> anyhow::Result<Connecting> {
        self.session_creator.create_new_connection(url).await
    }

    pub async fn accept(&self) -> Result<Accepting, AcceptError> {
        self.session_creator.accept_new_connection().await
    }

    /// Refuses every connection attempt at the transport level (QUIC
    /// CONNECTION_REFUSED, HTTP 503 for WebTransport and WebSocket) while
    /// `accept` is polled.
    /// A client learns at once that this endpoint is closing instead of waiting
    /// for its handshake to time out. Established sessions are unaffected.
    pub fn refuse_new_connections(&self) {
        self.session_creator
            .transport_creator
            .refuse_new_connections();
    }

    /// Resolves once the transport of every session of this endpoint has
    /// finished closing. Await it after dropping the sessions and before the
    /// process exits: the close is sent by a background task, so exiting
    /// first leaves the peer to detect the loss through its idle timeout.
    pub async fn wait_idle(&self) {
        self.session_creator.transport_creator.wait_idle().await;
    }

    /// The UDP port the endpoint is bound to; for `ServerConfig { port: 0, .. }`
    /// it is the port the OS picked. Fails on `BROWSER`, which has no socket.
    pub fn local_port(&self) -> anyhow::Result<u16> {
        self.session_creator.transport_creator.local_port()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CertificateVerification, ClientConfig, Endpoint, NATIVE,
        modules::moqt::domains::endpoint::ServerConfig,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, native_client, native_client_with_config, receive_disconnected,
            self_signed_server_config, spawn_native_server,
        },
    };

    async fn connect_trusting(
        server_config: ServerConfig,
        trusted_cert_path: String,
    ) -> anyhow::Result<()> {
        let server = Endpoint::<NATIVE>::create_server(&server_config).unwrap();
        let port = server.local_port().unwrap();
        let accept = tokio::spawn(async move {
            server
                .accept()
                .await?
                .await?
                .accept(&tracing::Span::current())
                .await
        });
        let client = native_client_with_config(ClientConfig {
            certificate_verification: CertificateVerification::TrustedCertificates(
                trusted_cert_path.into(),
            ),
            follow_go_away: false,
            ..ClientConfig::default()
        });
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            client
                .connect(&format!("moqt://127.0.0.1:{port}"))
                .await?
                .await
        })
        .await
        .expect("the handshake must end before its timeout");
        accept.abort();
        result.map(drop)
    }

    async fn run_client_that_drops_its_session_and_exits_after_wait_idle(url: String) {
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let endpoint = native_client();
                let connecting = endpoint.connect(&url).await.unwrap();
                let session = tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting)
                    .await
                    .unwrap()
                    .unwrap();
                drop(session);
                endpoint.wait_idle().await;
            })
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_client_connects_to_a_server_whose_certificate_it_trusts() {
        // Arrange
        let server_config = self_signed_server_config("trusted");
        let trusted_cert_path = server_config.cert_path.clone();

        // Act
        let result = connect_trusting(server_config, trusted_cert_path).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn a_client_refuses_a_server_whose_certificate_it_does_not_trust() {
        // Arrange
        let server_config = self_signed_server_config("untrusted");
        let trusted_cert_path = self_signed_server_config("untrusted-other").cert_path;

        // Act
        let result = connect_trusting(server_config, trusted_cert_path).await;

        // Assert
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("invalid peer certificate"), "{error}");
    }

    #[tokio::test]
    async fn a_refusing_server_turns_new_clients_away_at_once() {
        // Arrange
        let server_config = self_signed_server_config("refuse");
        let server = Endpoint::<NATIVE>::create_server(&server_config).unwrap();
        let port = server.local_port().unwrap();
        server.refuse_new_connections();
        let accept = tokio::spawn(async move {
            let _ = server.accept().await;
        });

        // Act
        let connect = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            native_client()
                .connect(&format!("moqt://127.0.0.1:{port}"))
                .await?
                .await
        })
        .await;

        // Assert
        let result = connect.expect("the refusal must arrive before the handshake timeout");
        assert!(result.is_err());
        accept.abort();
    }

    #[tokio::test]
    async fn server_observes_quic_client_exit_after_wait_idle() {
        // Arrange
        let (port, accept) = spawn_native_server("wait-idle-quic");

        // Act
        run_client_that_drops_its_session_and_exits_after_wait_idle(format!(
            "moqt://127.0.0.1:{port}"
        ))
        .await;

        // Assert
        let server = accept.await.unwrap();
        receive_disconnected(&server).await.unwrap();
    }

    #[tokio::test]
    async fn server_observes_web_socket_client_exit_after_wait_idle() {
        // Arrange
        let (port, accept) = spawn_native_server("wait-idle-ws");

        // Act
        run_client_that_drops_its_session_and_exits_after_wait_idle(format!(
            "wss://127.0.0.1:{port}/moq"
        ))
        .await;

        // Assert
        let server = accept.await.unwrap();
        receive_disconnected(&server).await.unwrap();
    }

    #[tokio::test]
    async fn server_observes_web_transport_client_exit_after_wait_idle() {
        // Arrange
        let (port, accept) = spawn_native_server("wait-idle-wt");

        // Act
        run_client_that_drops_its_session_and_exits_after_wait_idle(format!(
            "https://127.0.0.1:{port}/moq"
        ))
        .await;

        // Assert
        let server = accept.await.unwrap();
        receive_disconnected(&server).await.unwrap();
    }
}
