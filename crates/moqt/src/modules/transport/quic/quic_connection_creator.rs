use crate::modules::transport::transport_connection::BoxedConnection;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};

use quinn::rustls::{self};

use crate::modules::transport::{
    accept_error::AcceptError,
    application_protocol::ApplicationProtocols,
    client_crypto::{
        client_crypto, client_crypto_with_custom_cert, client_endpoint, quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    dual_stack_bind::bind_dual_stack,
    quic::quic_connection::QUICConnection,
    server_transport_config::quic_server_config,
    transport_connection_creator::TransportConnectionCreator,
};

pub struct QUICConnectionCreator {
    endpoint: quinn::Endpoint,
    refusing: AtomicBool,
}

impl QUICConnectionCreator {
    fn create_client(
        port_num: u16,
        crypto: rustls::ClientConfig,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        let mut endpoint = client_endpoint(port_num)?;
        endpoint.set_default_client_config(quic_client_config(crypto, protocols.quic_alpn_list())?);
        tracing::debug!(local_addr = ?endpoint.local_addr()?, "QUIC client endpoint ready");
        Ok(QUICConnectionCreator {
            endpoint,
            refusing: AtomicBool::new(false),
        })
    }
}

#[async_trait]
impl TransportConnectionCreator for QUICConnectionCreator {
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
        let server_config = quic_server_config(
            cert_path,
            key_path,
            keep_alive_sec,
            protocols.quic_alpn_list(),
        )?;
        let endpoint = bind_dual_stack(port_num, |address| {
            let endpoint = quinn::Endpoint::server(server_config, address)?;
            tracing::info!(%address, "QUIC server ready");
            Ok(endpoint)
        })?;
        Ok(QUICConnectionCreator {
            endpoint,
            refusing: AtomicBool::new(false),
        })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<BoxedConnection> {
        if target.transport != ClientTransport::Quic {
            anyhow::bail!("QUIC endpoint requires a moqt:// url, got {}", target.url);
        }
        let remote_address = target.resolve_remote_address().await?;
        let connecting = self
            .endpoint
            .connect(remote_address, &target.server_name())?;
        let connection = connecting.await?;

        Ok(Box::new(QUICConnection::new(connection)))
    }

    async fn accept_new_transport(&self) -> Result<BoxedConnection, AcceptError> {
        loop {
            let incoming = self
                .endpoint
                .accept()
                .await
                .ok_or(AcceptError::EndpointClosing)?;
            if self.refusing.load(Ordering::SeqCst) {
                incoming.refuse();
                continue;
            }
            let connection = incoming.await.map_err(anyhow::Error::from)?;
            return Ok(Box::new(QUICConnection::new(connection)));
        }
    }

    fn refuse_new_connections(&self) {
        self.refusing.store(true, Ordering::SeqCst);
    }

    async fn wait_idle(&self) {
        self.endpoint.wait_idle().await;
    }

    fn local_port(&self) -> anyhow::Result<u16> {
        Ok(self.endpoint.local_addr()?.port())
    }
}

#[cfg(test)]
mod tests {
    use super::QUICConnectionCreator;
    use crate::modules::{
        test_support::{HANDSHAKE_TIMEOUT, server_transport_creator},
        transport::{
            accept_error::AcceptError, transport_connection_creator::TransportConnectionCreator,
        },
    };

    #[tokio::test]
    async fn accept_on_a_closed_endpoint_reports_endpoint_closing() {
        // Arrange
        let server: QUICConnectionCreator = server_transport_creator("quic-closed");
        server.endpoint.close(0u32.into(), b"");

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, server.accept_new_transport()).await;

        // Assert
        assert!(matches!(result, Ok(Err(AcceptError::EndpointClosing))));
    }
}
