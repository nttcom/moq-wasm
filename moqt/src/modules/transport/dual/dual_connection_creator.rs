use std::{
    net::{Ipv6Addr, SocketAddr},
    sync::Arc,
};

use async_trait::async_trait;
use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use super::dual_connection::DualConnection;
use crate::modules::transport::{
    client_crypto::{
        MOQ_ALPN, client_crypto, client_crypto_with_custom_cert, client_endpoint,
        quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    crypto_provider::install_default_crypto_provider,
    quic::quic_connection::QUICConnection,
    transport_connection_creator::TransportConnectionCreator,
    webtransport::wt_connection::WtConnection,
};

enum DualEndpoint {
    Server(quinn::Endpoint),
    Client(Box<DualClient>),
}

/// One UDP socket serves both transports: raw QUIC connects with the
/// `moq-00` ALPN via `connect_with`, WebTransport reuses the same endpoint
/// through `web_transport_quinn::Client` with the `h3` ALPN.
struct DualClient {
    endpoint: quinn::Endpoint,
    quic_config: quinn::ClientConfig,
    web_transport: web_transport_quinn::Client,
}

pub struct DualProtocolCreator {
    endpoint: DualEndpoint,
}

impl DualProtocolCreator {
    fn create_client(port_num: u16, crypto: rustls::ClientConfig) -> anyhow::Result<Self> {
        let endpoint = client_endpoint(port_num)?;
        let quic_config = quic_client_config(crypto.clone(), MOQ_ALPN)?;
        let web_transport_config =
            quic_client_config(crypto, web_transport_quinn::ALPN.as_bytes())?;
        let web_transport =
            web_transport_quinn::Client::new(endpoint.clone(), web_transport_config);
        tracing::info!(
            "Client ready! for Dual Protocol: {:?}",
            endpoint.local_addr()?
        );
        Ok(DualProtocolCreator {
            endpoint: DualEndpoint::Client(Box::new(DualClient {
                endpoint,
                quic_config,
                web_transport,
            })),
        })
    }
}

#[async_trait]
impl TransportConnectionCreator for DualProtocolCreator {
    type Connection = DualConnection;

    fn client(port_num: u16, verify_certificate: bool) -> anyhow::Result<Self> {
        Self::create_client(port_num, client_crypto(verify_certificate)?)
    }

    fn client_with_custom_cert(port_num: u16, custom_cert_path: &str) -> anyhow::Result<Self> {
        Self::create_client(port_num, client_crypto_with_custom_cert(custom_cert_path)?)
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        install_default_crypto_provider();

        // 証明書を同期的に読み込む
        let cert = CertificateDer::pem_file_iter(cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificates failed: {:?}", e))?;

        // 秘密鍵を同期的に読み込む
        let key = PrivateKeyDer::from_pem_file(key_path)
            .inspect_err(|e| tracing::error!("Creating private key failed: {:?}", e.to_string()))?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .inspect_err(|e| tracing::error!("server config failed: {:?}", e.to_string()))?;

        // ALPN を2つ登録（WebTransport + QUIC）
        server_crypto.alpn_protocols = vec![
            web_transport_quinn::ALPN.as_bytes().to_vec(), // h3
            b"moq-00".to_vec(),
        ];
        server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
        ));
        let mut transport_config = quinn::TransportConfig::default();
        let keep_alive_sec = std::time::Duration::from_secs(keep_alive_sec);
        transport_config.keep_alive_interval(Some(keep_alive_sec));
        transport_config.max_concurrent_uni_streams(100000u32.into());
        transport_config.send_window(64 * 1024);
        transport_config.packet_threshold(5);
        transport_config.stream_receive_window(quinn::VarInt::from_u32(1024 * 1024));

        let transport_arc = Arc::new(transport_config);
        server_config.transport_config(transport_arc);

        let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port_num));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        tracing::info!("Server ready! for Dual Protocol: {:?}", address);

        Ok(DualProtocolCreator {
            endpoint: DualEndpoint::Server(endpoint),
        })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<Self::Connection> {
        let client = match &self.endpoint {
            DualEndpoint::Client(client) => client,
            DualEndpoint::Server(_) => {
                anyhow::bail!("Cannot create_new_transport on a server endpoint")
            }
        };
        match target.transport {
            ClientTransport::Quic => {
                let remote_address = target.resolve_remote_address().await?;
                let connection = client
                    .endpoint
                    .connect_with(
                        client.quic_config.clone(),
                        remote_address,
                        &target.server_name(),
                    )
                    .inspect_err(|e| tracing::error!("failed to connect: {:?}", e.to_string()))?
                    .await
                    .inspect_err(|e| {
                        tracing::error!("failed to create connection: {:?}", e.to_string())
                    })?;
                Ok(DualConnection::Quic(QUICConnection::new(connection)))
            }
            ClientTransport::WebTransport => {
                let session = client
                    .web_transport
                    .connect(target.url.clone())
                    .await
                    .inspect_err(|e| tracing::error!("failed to connect: {:?}", e))?;
                Ok(DualConnection::WebTransport(Box::new(WtConnection::new(
                    session,
                ))))
            }
        }
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        let endpoint = match &self.endpoint {
            DualEndpoint::Server(endpoint) => endpoint,
            DualEndpoint::Client(_) => {
                anyhow::bail!("Cannot accept_new_transport on a client endpoint")
            }
        };
        let incoming = endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("Endpoint is closing"))?;

        let connection = incoming
            .await
            .inspect_err(|e| tracing::error!("failed to create connection: {:?}", e.to_string()))?;

        // ALPN で分岐
        let alpn = connection
            .handshake_data()
            .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
            .and_then(|data| data.protocol)
            .ok_or_else(|| anyhow::anyhow!("No ALPN protocol negotiated"))?;

        if alpn.as_slice() == web_transport_quinn::ALPN.as_bytes() {
            // WebTransport: H3 ハンドシェイクを行いセッションを確立する
            let request = web_transport_quinn::Request::accept(connection)
                .await
                .inspect_err(|e| {
                    tracing::error!("failed to accept WebTransport request: {:?}", e)
                })?;
            let session = request.ok().await.inspect_err(|e| {
                tracing::error!("failed to establish WebTransport session: {:?}", e)
            })?;
            Ok(DualConnection::WebTransport(Box::new(WtConnection::new(
                session,
            ))))
        } else if alpn.as_slice() == b"moq-00" {
            // Raw QUIC
            Ok(DualConnection::Quic(QUICConnection::new(connection)))
        } else {
            anyhow::bail!("Unsupported ALPN: {:?}", String::from_utf8_lossy(&alpn))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::{connect_sessions, dual_client, spawn_dual_server};

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
    async fn dual_client_rejects_unknown_scheme() {
        // Act
        let result = dual_client().connect("http://127.0.0.1:1").await;

        // Assert
        assert!(result.is_err());
    }
}
