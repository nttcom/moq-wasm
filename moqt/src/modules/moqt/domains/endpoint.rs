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
    pub async fn connect(&self, url: &str) -> anyhow::Result<Connecting<T>> {
        self.session_creator.create_new_connection(url).await
    }

    pub async fn accept(&mut self) -> anyhow::Result<Accepting<T>> {
        self.session_creator.accept_new_connection().await
    }
}
