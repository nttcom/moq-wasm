use crate::{
    Accepting, Connecting, TransportProtocol,
    modules::{
        moqt::domains::session_creator::SessionCreator,
        transport::transport_connection_creator::TransportConnectionCreator,
    },
};

pub struct ClientConfig {
    pub port: u16,
    pub verify_certificate: bool,
    pub authorization_token: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: 0,
            verify_certificate: true,
            authorization_token: None,
        }
    }
}

pub struct ServerConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    // log_level: String,
}

pub struct Endpoint<T: TransportProtocol> {
    session_creator: SessionCreator<T>,
}

impl<T: TransportProtocol> Endpoint<T> {
    pub fn create_client(config: &ClientConfig) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client(config.port, config.verify_certificate)?;
        let session_creator = SessionCreator {
            transport_creator: client,
            authorization_token: config.authorization_token.clone(),
        };
        Ok(Self { session_creator })
    }

    pub fn create_client_with_custom_cert(
        port_num: u16,
        custom_cert_path: &str,
    ) -> anyhow::Result<Self> {
        let client = T::ConnectionCreator::client_with_custom_cert(port_num, custom_cert_path)?;
        let session_creator = SessionCreator {
            transport_creator: client,
            authorization_token: None,
        };
        Ok(Self { session_creator })
    }

    pub fn create_server(server_config: &ServerConfig) -> anyhow::Result<Self> {
        let server = T::ConnectionCreator::server(
            &server_config.cert_path,
            &server_config.key_path,
            server_config.port,
            server_config.keep_alive_interval_sec,
        )?;
        let session_creator = SessionCreator {
            transport_creator: server,
            authorization_token: None,
        };
        Ok(Self { session_creator })
    }

    /// `url` selects the transport by scheme: `moqt://host[:port]` for raw
    /// QUIC (default port 4433) and `https://host[:port][/path]` for
    /// WebTransport (default port 443). `QUIC` and `WEBTRANSPORT` endpoints
    /// accept only their own scheme; `DUAL` accepts both.
    pub async fn connect(&self, url: &str) -> anyhow::Result<Connecting> {
        self.session_creator.create_new_connection(url).await
    }

    pub async fn accept(&mut self) -> anyhow::Result<Accepting> {
        self.session_creator.accept_new_connection().await
    }

    /// Resolves once the transport of every session of this endpoint has
    /// finished closing. Await it after dropping the sessions and before the
    /// process exits: the close is sent by a background task, so exiting
    /// first leaves the peer to detect the loss through its idle timeout.
    pub async fn wait_idle(&self) {
        self.session_creator.transport_creator.wait_idle().await;
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::{
        HANDSHAKE_TIMEOUT, dual_client, receive_disconnected, spawn_dual_server,
    };

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
