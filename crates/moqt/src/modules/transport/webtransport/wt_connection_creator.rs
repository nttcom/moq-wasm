use crate::modules::transport::transport_connection::BoxedConnection;
use anyhow::Context;
use std::net::{Ipv6Addr, SocketAddr};

use async_trait::async_trait;
use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use web_transport_quinn::proto::{ConnectRequest, ConnectResponse};

use super::wt_connection::WtConnection;
use crate::modules::transport::{
    application_protocol::ApplicationProtocols,
    client_crypto::{
        client_crypto, client_crypto_with_custom_cert, client_endpoint, quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    crypto_provider::install_default_crypto_provider,
    transport_connection_creator::TransportConnectionCreator,
};

enum WtEndpoint {
    Server(tokio::sync::Mutex<web_transport_quinn::Server>),
    Client(quinn::ClientConfig),
}

pub struct WtConnectionCreator {
    endpoint: WtEndpoint,
    quic_endpoint: quinn::Endpoint,
    protocols: ApplicationProtocols,
}

impl WtConnectionCreator {
    fn create_client(
        port_num: u16,
        crypto: rustls::ClientConfig,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        let endpoint = client_endpoint(port_num)?;
        let client_config =
            quic_client_config(crypto, vec![web_transport_quinn::ALPN.as_bytes().to_vec()])?;
        tracing::debug!(local_addr = ?endpoint.local_addr()?, "WebTransport client endpoint ready");
        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Client(client_config),
            quic_endpoint: endpoint,
            protocols,
        })
    }
}

pub(crate) async fn connect_session(
    connection: quinn::Connection,
    target: &ConnectTarget,
    protocols: ApplicationProtocols,
) -> anyhow::Result<web_transport_quinn::Session> {
    let request = ConnectRequest::new(target.url.clone()).with_protocols(
        protocols
            .web_transport
            .iter()
            .map(|protocol| protocol.to_string()),
    );
    Ok(web_transport_quinn::Session::connect(connection, request).await?)
}

pub(crate) async fn accept_session(
    request: web_transport_quinn::Request,
    protocols: ApplicationProtocols,
) -> anyhow::Result<web_transport_quinn::Session> {
    let mut response = ConnectResponse::OK;
    response.protocol = protocols
        .select_web_transport(&request.protocols)
        .map(str::to_owned);
    Ok(request.respond(response).await?)
}

#[async_trait]
impl TransportConnectionCreator for WtConnectionCreator {
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
        _keep_alive_sec: u64,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        install_default_crypto_provider();

        // 証明書を同期的に読み込む
        let certs: Vec<_> = CertificateDer::pem_file_iter(cert_path)
            .context("failed to open the certificate file")?
            .collect::<Result<Vec<_>, _>>()
            .context("failed to parse the certificate file")?;

        // 秘密鍵を同期的に読み込む
        let key = PrivateKeyDer::from_pem_file(key_path)
            .context("failed to read the private key file")?;

        let addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port_num));
        let server = web_transport_quinn::ServerBuilder::new()
            .with_addr(addr)
            .with_certificate(certs, key)?;

        tracing::info!("Server ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            quic_endpoint: quinn::Endpoint::clone(&server),
            endpoint: WtEndpoint::Server(tokio::sync::Mutex::new(server)),
            protocols,
        })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<BoxedConnection> {
        let client_config = match &self.endpoint {
            WtEndpoint::Client(client_config) => client_config,
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
        let connection = target
            .connect_quic(&self.quic_endpoint, client_config.clone())
            .await?;
        let session = connect_session(connection, target, self.protocols).await?;

        Ok(Box::new(WtConnection::new(session)))
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<BoxedConnection> {
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

        let session = accept_session(request, self.protocols).await?;

        Ok(Box::new(WtConnection::new(session)))
    }

    async fn wait_idle(&self) {
        self.quic_endpoint.wait_idle().await;
    }

    fn local_port(&self) -> anyhow::Result<u16> {
        Ok(self.quic_endpoint.local_addr()?.port())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Endpoint, WEBTRANSPORT,
        modules::test_support::{connect_sessions_from, insecure_client_config, spawn_dual_server},
    };

    #[tokio::test]
    async fn web_transport_client_connects_to_a_host_name() {
        // Arrange
        let (port, accept) = spawn_dual_server("wt-localhost");
        let client = Endpoint::<WEBTRANSPORT>::create_client(&insecure_client_config()).unwrap();

        // Act
        let result =
            connect_sessions_from(&client, &format!("https://localhost:{port}/moq"), accept).await;

        // Assert
        result.unwrap();
    }
}
