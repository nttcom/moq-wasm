use crate::modules::transport::transport_connection::BoxedConnection;
use anyhow::Context;
use std::{
    net::{Ipv6Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use crate::modules::transport::{
    application_protocol::ApplicationProtocols,
    client_crypto::{
        client_crypto, client_crypto_with_custom_cert, client_endpoint, quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    crypto_provider::install_default_crypto_provider,
    quic::quic_connection::{QUICConnection, negotiated_alpn},
    server_transport_config::server_transport_config,
    transport_connection_creator::TransportConnectionCreator,
    webtransport::{
        wt_connection::WtConnection,
        wt_connection_creator::{accept_session, connect_session},
    },
};

enum DualEndpoint {
    Server(quinn::Endpoint),
    Client(Box<DualClient>),
}

impl DualEndpoint {
    fn quinn_endpoint(&self) -> &quinn::Endpoint {
        match self {
            Self::Server(endpoint) => endpoint,
            Self::Client(client) => &client.endpoint,
        }
    }
}

struct DualClient {
    endpoint: quinn::Endpoint,
    quic_config: quinn::ClientConfig,
    web_transport_config: quinn::ClientConfig,
}

pub struct DualProtocolCreator {
    endpoint: DualEndpoint,
    protocols: ApplicationProtocols,
    refusing: AtomicBool,
}

impl DualProtocolCreator {
    fn create_client(
        port_num: u16,
        crypto: rustls::ClientConfig,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        let endpoint = client_endpoint(port_num)?;
        let quic_config = quic_client_config(crypto.clone(), protocols.quic_alpn_list())?;
        let web_transport_config =
            quic_client_config(crypto, vec![web_transport_quinn::ALPN.as_bytes().to_vec()])?;
        tracing::debug!(local_addr = ?endpoint.local_addr()?, "Dual Protocol client endpoint ready");
        Ok(DualProtocolCreator {
            endpoint: DualEndpoint::Client(Box::new(DualClient {
                endpoint,
                quic_config,
                web_transport_config,
            })),
            protocols,
            refusing: AtomicBool::new(false),
        })
    }
}

#[async_trait]
impl TransportConnectionCreator for DualProtocolCreator {
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
        install_default_crypto_provider();

        // 証明書を同期的に読み込む
        let cert = CertificateDer::pem_file_iter(cert_path)
            .context("failed to open the certificate file")?
            .collect::<Result<Vec<_>, _>>()
            .context("failed to parse the certificate file")?;

        // 秘密鍵を同期的に読み込む
        let key = PrivateKeyDer::from_pem_file(key_path)
            .context("failed to read the private key file")?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .context("failed to build the TLS server config")?;

        server_crypto.alpn_protocols = [web_transport_quinn::ALPN.as_bytes().to_vec()]
            .into_iter()
            .chain(protocols.quic_alpn_list())
            .collect();
        server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
        ));
        let transport_arc = Arc::new(server_transport_config(keep_alive_sec));
        server_config.transport_config(transport_arc);

        let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port_num));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        tracing::info!("Server ready! for Dual Protocol: {:?}", address);

        Ok(DualProtocolCreator {
            endpoint: DualEndpoint::Server(endpoint),
            protocols,
            refusing: AtomicBool::new(false),
        })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<BoxedConnection> {
        let client = match &self.endpoint {
            DualEndpoint::Client(client) => client,
            DualEndpoint::Server(_) => {
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
        }
    }

    async fn accept_new_transport(&self) -> anyhow::Result<BoxedConnection> {
        let endpoint = match &self.endpoint {
            DualEndpoint::Server(endpoint) => endpoint,
            DualEndpoint::Client(_) => {
                anyhow::bail!("Cannot accept_new_transport on a client endpoint")
            }
        };
        let connection = loop {
            let incoming = endpoint
                .accept()
                .await
                .ok_or_else(|| anyhow::anyhow!("Endpoint is closing"))?;
            if self.refusing.load(Ordering::SeqCst) {
                incoming.refuse();
                continue;
            }
            break incoming.await?;
        };

        let alpn = negotiated_alpn(&connection)
            .ok_or_else(|| anyhow::anyhow!("No ALPN protocol negotiated"))?;

        if alpn == web_transport_quinn::ALPN {
            let request = web_transport_quinn::Request::accept(connection).await?;
            let session = accept_session(request, self.protocols).await?;
            Ok(Box::new(WtConnection::new(session)))
        } else if self.protocols.quic_alpns.contains(&alpn.as_str()) {
            Ok(Box::new(QUICConnection::new(connection)))
        } else {
            anyhow::bail!("Unsupported ALPN: {alpn:?}")
        }
    }

    fn refuse_new_connections(&self) {
        self.refusing.store(true, Ordering::SeqCst);
    }

    async fn wait_idle(&self) {
        self.endpoint.quinn_endpoint().wait_idle().await;
    }

    fn local_port(&self) -> anyhow::Result<u16> {
        Ok(self.endpoint.quinn_endpoint().local_addr()?.port())
    }
}

#[cfg(test)]
mod tests {
    use super::DualProtocolCreator;
    use crate::modules::{
        test_support::{
            HANDSHAKE_TIMEOUT, connect_sessions, dual_client, self_signed_server_config,
            spawn_dual_server,
        },
        transport::{
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
        let server = DualProtocolCreator::server(
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
        let client = DualProtocolCreator::client(0, false, client).unwrap();
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
            "dual-alpn",
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
            "dual-wt-protocol",
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
            "dual-wt-no-protocol",
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
    async fn dual_client_connects_over_raw_quic_with_moqt_scheme() {
        // Arrange
        let (port, accept) = spawn_dual_server("dual-quic");

        // Act
        let result = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn dual_client_connects_over_web_transport_with_https_scheme() {
        // Arrange
        let (port, accept) = spawn_dual_server("dual-wt");

        // Act
        let result = connect_sessions(&format!("https://127.0.0.1:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn dual_client_connects_over_web_transport_to_a_host_name() {
        // Arrange
        let (port, accept) = spawn_dual_server("dual-wt-localhost");

        // Act
        let result = connect_sessions(&format!("https://localhost:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }

    #[tokio::test]
    async fn dual_client_rejects_unknown_scheme() {
        // Act
        let result = dual_client().connect("http://127.0.0.1:1").await;

        // Assert
        assert!(result.is_err());
    }
}
