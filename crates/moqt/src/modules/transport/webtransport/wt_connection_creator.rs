use crate::modules::transport::transport_connection::BoxedConnection;
use anyhow::Context;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use web_transport_quinn::proto::{ConnectRequest, ConnectResponse};

use super::wt_connection::WtConnection;
use crate::modules::transport::{
    accept_error::AcceptError,
    application_protocol::ApplicationProtocols,
    client_crypto::{
        client_crypto, client_crypto_with_custom_cert, client_endpoint, quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    crypto_provider::install_default_crypto_provider,
    dual_stack_bind::bind_dual_stack,
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
    refusing: AtomicBool,
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
            refusing: AtomicBool::new(false),
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

        let server = bind_dual_stack(port_num, |address| {
            Ok(web_transport_quinn::ServerBuilder::new()
                .with_addr(address)
                .with_certificate(certs, key)?)
        })?;

        tracing::info!("Server ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            quic_endpoint: quinn::Endpoint::clone(&server),
            endpoint: WtEndpoint::Server(tokio::sync::Mutex::new(server)),
            protocols,
            refusing: AtomicBool::new(false),
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

    async fn accept_new_transport(&self) -> Result<BoxedConnection, AcceptError> {
        let server = match &self.endpoint {
            WtEndpoint::Server(s) => s,
            WtEndpoint::Client(_) => Err(anyhow::anyhow!(
                "Cannot accept_new_transport on a client endpoint"
            ))?,
        };

        loop {
            let Some(request) = server.lock().await.accept().await else {
                return Err(AcceptError::EndpointClosing);
            };
            if self.refusing.load(Ordering::SeqCst) {
                let _ = request
                    .reject(web_transport_quinn::http::StatusCode::SERVICE_UNAVAILABLE)
                    .await;
                continue;
            }
            let session = accept_session(request, self.protocols).await?;
            return Ok(Box::new(WtConnection::new(session)));
        }
    }

    fn refuse_new_connections(&self) {
        self.refusing.store(true, Ordering::SeqCst);
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
    use super::WtConnectionCreator;
    use crate::{
        Endpoint, WEBTRANSPORT,
        modules::{
            test_support::{
                HANDSHAKE_TIMEOUT, connect_sessions_from, insecure_client_config,
                server_transport_creator, spawn_dual_server,
            },
            transport::{
                accept_error::AcceptError, transport_connection_creator::TransportConnectionCreator,
            },
        },
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

    #[tokio::test]
    async fn accept_on_a_closed_endpoint_reports_endpoint_closing() {
        // Arrange
        let server: WtConnectionCreator = server_transport_creator("wt-closed");
        server.quic_endpoint.close(0u32.into(), b"");

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, server.accept_new_transport()).await;

        // Assert
        assert!(matches!(result, Ok(Err(AcceptError::EndpointClosing))));
    }
}
