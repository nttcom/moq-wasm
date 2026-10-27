use std::sync::Arc;

use crate::{
    Connecting, TransportProtocol,
    modules::{
        moqt::{
            domains::session_creator::SessionCreator,
            version::{
                DEFAULT_VERSIONS, Version, application_protocols, server_application_protocols,
            },
        },
        transport::{
            accept_error::AcceptError, transport_connection_creator::TransportConnectionCreator,
        },
    },
    server::Accepting,
};

pub struct ClientConfig {
    pub port: u16,
    pub verify_certificate: bool,
    pub authorization_token: Option<String>,
    /// A session follows the server's GOAWAY by itself (draft-14 §3.5):
    /// it connects to the New Session URI, sends its subscriptions there
    /// again and keeps the request ids and track aliases the application
    /// knows. With `false`, GOAWAY arrives as a `SessionEvent` and the session
    /// stays on its connection.
    pub follow_go_away: bool,
    /// The drafts offered to the server, most preferred first; the server
    /// picks one of them. `create_client` fails when it is empty.
    pub versions: Vec<Version>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: 0,
            verify_certificate: true,
            authorization_token: None,
            follow_go_away: true,
            versions: DEFAULT_VERSIONS.to_vec(),
        }
    }
}

pub struct ServerConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    /// The drafts accepted from clients, most preferred first: of those a
    /// client offers, the first one listed here is chosen. A WebTransport
    /// session without a subprotocol is draft-14, so with draft-14 listed
    /// first every WebTransport session is draft-14. `create_server` fails
    /// when it is empty.
    pub versions: Vec<Version>,
}

pub struct Endpoint<T: TransportProtocol> {
    session_creator: Arc<SessionCreator<T>>,
}

impl<T: TransportProtocol> Endpoint<T> {
    pub fn create_client(config: &ClientConfig) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client(
            config.port,
            config.verify_certificate,
            application_protocols(&config.versions)?,
        )?;
        let session_creator = SessionCreator {
            transport_creator: client,
            authorization_token: config.authorization_token.clone(),
            follow_go_away: config.follow_go_away,
            versions: config.versions.clone(),
        };
        Ok(Self {
            session_creator: Arc::new(session_creator),
        })
    }

    pub fn create_client_with_custom_cert(
        port_num: u16,
        custom_cert_path: &str,
    ) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client_with_custom_cert(
            port_num,
            custom_cert_path,
            application_protocols(DEFAULT_VERSIONS)?,
        )?;
        let session_creator = SessionCreator {
            transport_creator: client,
            authorization_token: None,
            follow_go_away: true,
            versions: DEFAULT_VERSIONS.to_vec(),
        };
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
            server_application_protocols(&server_config.versions)?,
        )?;
        let session_creator = SessionCreator {
            transport_creator: server,
            authorization_token: None,
            follow_go_away: false,
            versions: server_config.versions.clone(),
        };
        Ok(Self {
            session_creator: Arc::new(session_creator),
        })
    }

    /// `url` selects the transport by scheme: `moqt://host[:port]` for raw
    /// QUIC (default port 4433) and `https://host[:port][/path]` for
    /// WebTransport (default port 443). `QUIC` and `WEBTRANSPORT` endpoints
    /// accept only their own scheme; `DUAL` accepts both.
    pub async fn connect(&self, url: &str) -> anyhow::Result<Connecting> {
        self.session_creator.create_new_connection(url).await
    }

    pub async fn accept(&self) -> Result<Accepting, AcceptError> {
        self.session_creator.accept_new_connection().await
    }

    /// Refuses every connection attempt at the transport level (QUIC
    /// CONNECTION_REFUSED, HTTP 503 for WebTransport) while `accept` is polled.
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
        ClientConfig, DUAL, Endpoint, Session, Version,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, connect_sessions_from, dual_client, insecure_client_config,
            receive_disconnected, self_signed_server_config, spawn_dual_server,
        },
        server::ServerConfig,
    };

    /// `url_of(port)` names the server's port in a `moqt` or `https` URL.
    async fn session_version(
        server_versions: &[Version],
        client_versions: &[Version],
        url_of: fn(u16) -> String,
    ) -> Version {
        let server = Endpoint::<DUAL>::create_server(&ServerConfig {
            versions: server_versions.to_vec(),
            ..self_signed_server_config("versions")
        })
        .unwrap();
        let port = server.local_port().unwrap();
        let accept: tokio::task::JoinHandle<Session> = tokio::spawn(async move {
            server
                .accept()
                .await
                .unwrap()
                .await
                .unwrap()
                .accept()
                .await
                .unwrap()
        });
        let client = Endpoint::<DUAL>::create_client(&ClientConfig {
            versions: client_versions.to_vec(),
            ..insecure_client_config()
        })
        .unwrap();
        let (client, _server) = connect_sessions_from(&client, &url_of(port), accept)
            .await
            .unwrap();
        client.context().connection().control.version()
    }

    fn quic_url(port: u16) -> String {
        format!("moqt://127.0.0.1:{port}")
    }

    fn web_transport_url(port: u16) -> String {
        format!("https://127.0.0.1:{port}/moq")
    }

    #[tokio::test]
    async fn a_server_accepting_both_drafts_speaks_the_one_each_client_offers() {
        // Arrange
        let both = [Version::Draft14, Version::Draft18];

        // Act
        let draft14 = session_version(&both, &[Version::Draft14], quic_url).await;
        let draft18 = session_version(&both, &[Version::Draft18], quic_url).await;

        // Assert
        assert_eq!(draft14, Version::Draft14);
        assert_eq!(draft18, Version::Draft18);
    }

    #[tokio::test]
    async fn the_server_preference_decides_when_the_client_offers_both() {
        // Act
        let version = session_version(
            &[Version::Draft18, Version::Draft14],
            &[Version::Draft14, Version::Draft18],
            quic_url,
        )
        .await;

        // Assert
        assert_eq!(version, Version::Draft18);
    }

    #[tokio::test]
    async fn the_server_preference_decides_over_web_transport_when_the_client_offers_both() {
        // Arrange
        let both = [Version::Draft14, Version::Draft18];

        // Act
        let draft14_first = session_version(&both, &both, web_transport_url).await;
        let draft18_first = session_version(
            &[Version::Draft18, Version::Draft14],
            &both,
            web_transport_url,
        )
        .await;

        // Assert
        assert_eq!(draft14_first, Version::Draft14);
        assert_eq!(draft18_first, Version::Draft18);
    }

    #[tokio::test]
    async fn a_draft18_only_server_refuses_a_web_transport_session_without_a_subprotocol() {
        // Arrange: a WebTransport client offering draft-14 asks for no subprotocol
        let server = Endpoint::<DUAL>::create_server(&ServerConfig {
            versions: vec![Version::Draft18],
            ..self_signed_server_config("draft18-only-web-transport")
        })
        .unwrap();
        let port = server.local_port().unwrap();
        let accept = tokio::spawn(async move { server.accept().await.unwrap().await.map(|_| ()) });
        let client = Endpoint::<DUAL>::create_client(&ClientConfig {
            versions: vec![Version::Draft14],
            ..insecure_client_config()
        })
        .unwrap();

        // Act
        let url = format!("https://127.0.0.1:{port}/moq");
        let connecting = tokio::spawn(async move {
            let _session = client.connect(&url).await?.await;
            anyhow::Ok(())
        });
        let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();
        connecting.abort();

        // Assert
        assert!(accepted.is_err());
    }

    #[test]
    fn a_client_offering_no_version_is_not_created() {
        // Act
        let client = Endpoint::<DUAL>::create_client(&ClientConfig {
            versions: Vec::new(),
            ..insecure_client_config()
        });

        // Assert
        assert!(client.is_err());
    }

    async fn run_client_that_drops_its_session_and_exits_after_wait_idle(url: String) {
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let endpoint = dual_client();
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
    async fn a_refusing_server_turns_new_clients_away_at_once() {
        // Arrange
        let server_config = self_signed_server_config("refuse");
        let server = Endpoint::<DUAL>::create_server(&server_config).unwrap();
        let port = server.local_port().unwrap();
        server.refuse_new_connections();
        let accept = tokio::spawn(async move {
            let _ = server.accept().await;
        });

        // Act
        let connect = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            dual_client()
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
        let (port, accept) = spawn_dual_server("wait-idle-quic");

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
    async fn server_observes_web_transport_client_exit_after_wait_idle() {
        // Arrange
        let (port, accept) = spawn_dual_server("wait-idle-wt");

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
