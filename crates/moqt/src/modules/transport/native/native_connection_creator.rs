use crate::modules::transport::transport_connection::BoxedConnection;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use quinn::rustls::{self};

use crate::modules::transport::{
    accept_error::AcceptError,
    application_protocol::ApplicationProtocols,
    client_crypto::{
        client_crypto, client_crypto_with_custom_cert, client_endpoint, quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    dual_stack_bind::bind_dual_stack,
    quic::quic_connection::{QUICConnection, negotiated_alpn},
    server_transport_config::quic_server_config,
    transport_connection_creator::TransportConnectionCreator,
    websocket::{
        HTTP11_ALPN, LiveConnections, websocket_client, websocket_listener::WebSocketListener,
    },
    webtransport::{
        wt_connection::WtConnection,
        wt_connection_creator::{accept_session, connect_session},
    },
};

enum NativeEndpoint {
    Server(Box<NativeServer>),
    Client(Box<NativeClient>),
}

impl NativeEndpoint {
    fn quinn_endpoint(&self) -> &quinn::Endpoint {
        match self {
            Self::Server(server) => &server.quic,
            Self::Client(client) => &client.endpoint,
        }
    }
}

/// QUIC and WebTransport on the UDP port, QMux over WebSocket on the TCP
/// port of the same number.
struct NativeServer {
    quic: quinn::Endpoint,
    web_socket: WebSocketListener,
}

struct NativeClient {
    endpoint: quinn::Endpoint,
    quic_config: quinn::ClientConfig,
    web_transport_config: quinn::ClientConfig,
    web_socket_crypto: Arc<rustls::ClientConfig>,
}

/// How many ephemeral UDP ports are drawn when the number is taken on TCP.
const EPHEMERAL_PORT_ATTEMPTS: u32 = 8;

pub struct NativeConnectionCreator {
    endpoint: NativeEndpoint,
    protocols: ApplicationProtocols,
    refusing: Arc<AtomicBool>,
    web_socket_sessions: Arc<LiveConnections>,
}

impl NativeConnectionCreator {
    fn create_client(
        port_num: u16,
        crypto: rustls::ClientConfig,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        let endpoint = client_endpoint(port_num)?;
        let quic_config = quic_client_config(crypto.clone(), protocols.quic_alpn_list())?;
        let web_transport_config = quic_client_config(
            crypto.clone(),
            vec![web_transport_quinn::ALPN.as_bytes().to_vec()],
        )?;
        let mut web_socket_crypto = crypto;
        web_socket_crypto.alpn_protocols = vec![HTTP11_ALPN.to_vec()];
        tracing::debug!(local_addr = ?endpoint.local_addr()?, "native client endpoint ready");
        Ok(NativeConnectionCreator {
            endpoint: NativeEndpoint::Client(Box::new(NativeClient {
                endpoint,
                quic_config,
                web_transport_config,
                web_socket_crypto: Arc::new(web_socket_crypto),
            })),
            protocols,
            refusing: Arc::new(AtomicBool::new(false)),
            web_socket_sessions: Arc::default(),
        })
    }
}

#[async_trait]
impl TransportConnectionCreator for NativeConnectionCreator {
    const WEB_SOCKET_HEAD_START: Option<Duration> = Some(Duration::from_millis(200));

    fn client(
        port_num: u16,
        verify_certificate: bool,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        Self::create_client(port_num, client_crypto(verify_certificate)?, protocols)
    }

    fn client_with_custom_cert(
        port_num: u16,
        custom_cert_path: &str,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        Self::create_client(
            port_num,
            client_crypto_with_custom_cert(custom_cert_path)?,
            protocols,
        )
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        let alpn_protocols = [web_transport_quinn::ALPN.as_bytes().to_vec()]
            .into_iter()
            .chain(protocols.quic_alpn_list())
            .collect();
        let server_config =
            quic_server_config(cert_path, key_path, keep_alive_sec, alpn_protocols)?;

        let refusing = Arc::new(AtomicBool::new(false));
        // Port 0 lets UDP pick the number and TCP then takes the same one. The
        // number UDP picks is often held on TCP already, so a new one is drawn.
        let mut attempts = 0;
        let (quic, web_socket) = loop {
            let server_config = server_config.clone();
            let quic = bind_dual_stack(port_num, |address| {
                let endpoint = quinn::Endpoint::server(server_config, address)?;
                tracing::info!(%address, "QUIC and WebTransport server ready");
                Ok(endpoint)
            })?;
            match WebSocketListener::bind(
                quic.local_addr()?.port(),
                cert_path,
                key_path,
                protocols,
                refusing.clone(),
            ) {
                Ok(web_socket) => break (quic, web_socket),
                Err(error) if port_num == 0 && attempts < EPHEMERAL_PORT_ATTEMPTS => {
                    tracing::debug!(%error, "drawing another ephemeral port for WebSocket");
                    attempts += 1;
                }
                Err(error) => return Err(error),
            }
        };

        Ok(NativeConnectionCreator {
            endpoint: NativeEndpoint::Server(Box::new(NativeServer { quic, web_socket })),
            protocols,
            refusing,
            web_socket_sessions: Arc::default(),
        })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<BoxedConnection> {
        let client = match &self.endpoint {
            NativeEndpoint::Client(client) => client,
            NativeEndpoint::Server(_) => {
                anyhow::bail!("Cannot create_new_transport on a server endpoint")
            }
        };
        match target.transport {
            ClientTransport::Quic => {
                let connection = target
                    .connect_quic(&client.endpoint, client.quic_config.clone())
                    .await?;
                Ok(Box::new(QUICConnection::new(connection)))
            }
            ClientTransport::WebTransport => {
                let connection = target
                    .connect_quic(&client.endpoint, client.web_transport_config.clone())
                    .await?;
                let session = connect_session(connection, target, self.protocols).await?;
                Ok(Box::new(WtConnection::new(session)))
            }
            ClientTransport::WebSocket => {
                let connection = websocket_client::connect(
                    target,
                    client.web_socket_crypto.clone(),
                    self.protocols,
                )
                .await?;
                self.web_socket_sessions.track(connection.transport_ended());
                Ok(Box::new(connection))
            }
        }
    }

    async fn accept_new_transport(&self) -> Result<BoxedConnection, AcceptError> {
        let server = match &self.endpoint {
            NativeEndpoint::Server(server) => server,
            NativeEndpoint::Client(_) => Err(anyhow::anyhow!(
                "Cannot accept_new_transport on a client endpoint"
            ))?,
        };
        let connection = loop {
            tokio::select! {
                incoming = server.quic.accept() => {
                    let incoming = incoming.ok_or(AcceptError::EndpointClosing)?;
                    if self.refusing.load(Ordering::SeqCst) {
                        incoming.refuse();
                        continue;
                    }
                    break incoming.await.map_err(anyhow::Error::from)?;
                }
                accepted = server.web_socket.accept() => {
                    let connection = accepted?;
                    self.web_socket_sessions.track(connection.transport_ended());
                    return Ok(Box::new(connection));
                }
            }
        };

        let alpn = negotiated_alpn(&connection)
            .ok_or_else(|| anyhow::anyhow!("No ALPN protocol negotiated"))?;

        if alpn == web_transport_quinn::ALPN {
            let request = web_transport_quinn::Request::accept(connection)
                .await
                .map_err(anyhow::Error::from)?;
            let session = accept_session(request, self.protocols).await?;
            Ok(Box::new(WtConnection::new(session)))
        } else if self.protocols.quic_alpns.contains(&alpn.as_str()) {
            Ok(Box::new(QUICConnection::new(connection)))
        } else {
            Err(anyhow::anyhow!("Unsupported ALPN: {alpn:?}"))?
        }
    }

    fn refuse_new_connections(&self) {
        self.refusing.store(true, Ordering::SeqCst);
    }

    async fn wait_idle(&self) {
        self.endpoint.quinn_endpoint().wait_idle().await;
        self.web_socket_sessions.wait_idle().await;
    }

    fn local_port(&self) -> anyhow::Result<u16> {
        Ok(self.endpoint.quinn_endpoint().local_addr()?.port())
    }
}

#[cfg(test)]
mod tests {
    use super::NativeConnectionCreator;
    use crate::modules::{
        moqt::version::APPLICATION_PROTOCOLS,
        test_support::{
            HANDSHAKE_TIMEOUT, connect_sessions, native_client, self_signed_server_config,
            server_transport_creator, spawn_native_server,
        },
        transport::{
            accept_error::AcceptError,
            application_protocol::{ApplicationProtocols, NegotiatedProtocol},
            connect_target::ConnectTarget,
            transport_connection::BoxedConnection,
            transport_connection_creator::TransportConnectionCreator,
        },
    };

    /// Returns the (client, server) connections.
    async fn connect_transports(
        name: &str,
        scheme: &str,
        path: &str,
        server: ApplicationProtocols,
        client: ApplicationProtocols,
    ) -> (BoxedConnection, BoxedConnection) {
        let config = self_signed_server_config(name);
        let server = NativeConnectionCreator::server(
            &config.cert_path,
            &config.key_path,
            0,
            config.keep_alive_interval_sec,
            server,
        )
        .unwrap();
        let url = format!(
            "{scheme}://127.0.0.1:{}{path}",
            server.local_port().unwrap()
        );
        let client = NativeConnectionCreator::client(0, false, client).unwrap();
        let target = ConnectTarget::parse(&url).unwrap();
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let (client, server) = tokio::join!(
                client.create_new_transport(&target),
                server.accept_new_transport()
            );
            (client.unwrap(), server.unwrap())
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn raw_quic_connections_report_the_alpn_they_negotiated() {
        // Act
        let (client, server) = connect_transports(
            "native-alpn",
            "moqt",
            "",
            ApplicationProtocols {
                quic_alpns: &["moqt-b", "moqt-a"],
                web_transport: &[],
            },
            ApplicationProtocols {
                quic_alpns: &["moqt-a"],
                web_transport: &[],
            },
        )
        .await;

        // Assert
        let expected = NegotiatedProtocol::Quic {
            alpn: "moqt-a".to_string(),
        };
        assert_eq!(client.negotiated_protocol(), expected);
        assert_eq!(server.negotiated_protocol(), expected);
    }

    #[tokio::test]
    async fn web_transport_sessions_carry_the_subprotocol_the_server_prefers() {
        // Act
        let (client, server) = connect_transports(
            "native-wt-protocol",
            "https",
            "/moq",
            ApplicationProtocols {
                quic_alpns: &[],
                web_transport: &["moqt-b", "moqt-a"],
            },
            ApplicationProtocols {
                quic_alpns: &[],
                web_transport: &["moqt-a", "moqt-b"],
            },
        )
        .await;

        // Assert
        let expected = NegotiatedProtocol::WebTransport {
            protocol: Some("moqt-b".to_string()),
        };
        assert_eq!(client.negotiated_protocol(), expected);
        assert_eq!(server.negotiated_protocol(), expected);
    }

    #[tokio::test]
    async fn web_transport_sessions_without_a_shared_subprotocol_carry_none() {
        // Act
        let (client, server) = connect_transports(
            "native-wt-no-protocol",
            "https",
            "/moq",
            ApplicationProtocols {
                quic_alpns: &[],
                web_transport: &["moqt-a"],
            },
            ApplicationProtocols {
                quic_alpns: &[],
                web_transport: &["moqt-b"],
            },
        )
        .await;

        // Assert
        let expected = NegotiatedProtocol::WebTransport { protocol: None };
        assert_eq!(client.negotiated_protocol(), expected);
        assert_eq!(server.negotiated_protocol(), expected);
    }

    #[tokio::test]
    async fn native_client_connects_over_raw_quic_with_moqt_scheme() {
        // Arrange
        let (port, accept) = spawn_native_server("native-quic");

        // Act
        let result = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn native_client_connects_over_web_transport_with_https_scheme() {
        // Arrange
        let (port, accept) = spawn_native_server("native-wt");

        // Act
        let result = connect_sessions(&format!("https://127.0.0.1:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn web_socket_sessions_carry_the_alpn_the_subprotocol_names() {
        // Act
        let (client, server) = connect_transports(
            "native-ws-alpn",
            "wss",
            "/moq",
            ApplicationProtocols {
                quic_alpns: &["moq-00"],
                web_transport: &[],
            },
            ApplicationProtocols {
                quic_alpns: &["moq-00"],
                web_transport: &[],
            },
        )
        .await;

        // Assert
        let expected = NegotiatedProtocol::WebSocket {
            alpn: Some("moq-00".to_string()),
        };
        assert_eq!(client.negotiated_protocol(), expected);
        assert_eq!(server.negotiated_protocol(), expected);
        assert!(client.addresses().remote.is_some());
    }

    #[tokio::test]
    async fn native_client_connects_over_web_socket_with_wss_scheme() {
        // Arrange
        let (port, accept) = spawn_native_server("native-ws");

        // Act
        let result = connect_sessions(&format!("wss://127.0.0.1:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn native_client_connects_over_web_socket_to_a_host_name() {
        // Arrange
        let (port, accept) = spawn_native_server("native-ws-localhost");

        // Act
        let result = connect_sessions(&format!("wss://localhost:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn a_dropped_server_frees_its_web_socket_port() {
        // Arrange
        let config = self_signed_server_config("native-ws-rebind");
        let server = NativeConnectionCreator::server(
            &config.cert_path,
            &config.key_path,
            0,
            config.keep_alive_interval_sec,
            APPLICATION_PROTOCOLS,
        )
        .unwrap();
        let port = server.local_port().unwrap();

        // Act: the aborted accept task releases the listener on its next poll
        drop(server);
        tokio::task::yield_now().await;
        let rebound = NativeConnectionCreator::server(
            &config.cert_path,
            &config.key_path,
            port,
            config.keep_alive_interval_sec,
            APPLICATION_PROTOCOLS,
        );

        // Assert
        rebound.unwrap();
    }

    #[tokio::test]
    async fn a_refusing_server_answers_the_web_socket_upgrade_with_503() {
        // Arrange
        let server: NativeConnectionCreator = server_transport_creator("native-ws-refuse");
        let port = server.local_port().unwrap();
        server.refuse_new_connections();
        let client = NativeConnectionCreator::client(0, false, APPLICATION_PROTOCOLS).unwrap();
        let target = ConnectTarget::parse(&format!("wss://127.0.0.1:{port}/moq")).unwrap();

        // Act
        let error = tokio::time::timeout(HANDSHAKE_TIMEOUT, client.create_new_transport(&target))
            .await
            .unwrap()
            .unwrap_err();

        // Assert
        assert!(format!("{error:#}").contains("503"), "{error:#}");
    }

    #[tokio::test]
    async fn native_client_connects_over_web_transport_to_a_host_name() {
        // Arrange
        let (port, accept) = spawn_native_server("native-wt-localhost");

        // Act
        let result = connect_sessions(&format!("https://localhost:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn native_client_rejects_unknown_scheme() {
        // Act
        let result = native_client().connect("http://127.0.0.1:1").await;

        // Assert
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn accept_on_a_closed_endpoint_reports_endpoint_closing() {
        // Arrange
        let server: NativeConnectionCreator = server_transport_creator("native-closed");
        server.endpoint.quinn_endpoint().close(0u32.into(), b"");

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, server.accept_new_transport()).await;

        // Assert
        assert!(matches!(result, Ok(Err(AcceptError::EndpointClosing))));
    }
}
