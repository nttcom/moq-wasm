use std::net::{Ipv6Addr, SocketAddr};

use async_trait::async_trait;
use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use super::wt_connection::WtConnection;
use crate::modules::transport::{
    client_crypto::{
        client_crypto, client_crypto_with_custom_cert, client_endpoint, quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    crypto_provider::install_default_crypto_provider,
    transport_connection_creator::TransportConnectionCreator,
};

enum WtEndpoint {
    Server(tokio::sync::Mutex<web_transport_quinn::Server>),
    Client(web_transport_quinn::Client),
}

pub struct WtConnectionCreator {
    endpoint: WtEndpoint,
}

impl WtConnectionCreator {
    fn create_client(port_num: u16, crypto: rustls::ClientConfig) -> anyhow::Result<Self> {
        let endpoint = client_endpoint(port_num)?;
        let client_config = quic_client_config(crypto, web_transport_quinn::ALPN.as_bytes())?;
        tracing::info!(
            "Client ready! for WebTransport: {:?}",
            endpoint.local_addr()?
        );
        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Client(web_transport_quinn::Client::new(endpoint, client_config)),
        })
    }
}

#[async_trait]
impl TransportConnectionCreator for WtConnectionCreator {
    type Connection = WtConnection;

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
        _keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        install_default_crypto_provider();

        // 証明書を同期的に読み込む
        let certs: Vec<_> = CertificateDer::pem_file_iter(cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificates failed: {:?}", e))?;

        // 秘密鍵を同期的に読み込む
        let key = PrivateKeyDer::from_pem_file(key_path)
            .inspect_err(|e| tracing::error!("Creating private key failed: {:?}", e.to_string()))?;

        let addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port_num));
        let server = web_transport_quinn::ServerBuilder::new()
            .with_addr(addr)
            .with_certificate(certs, key)?;

        tracing::info!("Server ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Server(tokio::sync::Mutex::new(server)),
        })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<Self::Connection> {
        let client = match &self.endpoint {
            WtEndpoint::Client(c) => c,
            WtEndpoint::Server(_) => {
                anyhow::bail!("Cannot create_new_transport on a server endpoint")
            }
        };
        if target.transport != ClientTransport::WebTransport {
            anyhow::bail!(
                "WebTransport endpoint requires an https:// url, got {}",
                target.url
            );
        }
        let session = client
            .connect(target.url.clone())
            .await
            .inspect_err(|e| tracing::error!("failed to connect: {:?}", e))?;

        Ok(WtConnection::new(session))
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        let server = match &self.endpoint {
            WtEndpoint::Server(s) => s,
            WtEndpoint::Client(_) => {
                anyhow::bail!("Cannot accept_new_transport on a client endpoint")
            }
        };

        // クライアントの接続を待つ
        let Some(request) = server.lock().await.accept().await else {
            anyhow::bail!("Server endpoint closed");
        };

        // リクエストを受け入れてセッションを確立する
        let session = request
            .ok()
            .await
            .inspect_err(|e| tracing::error!("failed to establish session: {:?}", e))?;

        Ok(WtConnection::new(session))
    }
}
