use std::sync::Arc;

use crate::modules::moqt::{
    moqt_connection::MOQTConnection, moqt_connection_creator::MOQTConnectionCreator,
};

use crate::modules::transport::quic::quic_connection_creator::QUICConnectionCreator;

mod modules;

pub struct MOQTConfig {
    port: u16,
    cert_path: String,
    key_path: String,
    keep_alive_interval_sec: u64,
    // use_webtransport: bool,
    log_level: String,
}

impl MOQTConfig {
    pub fn new(
        port: u16,
        cert_path: String,
        key_path: String,
        keep_alive_interval_sec: u64,
        // use_webtransport: bool,
        log_level: String,
    ) -> Self {
        Self {
            port,
            cert_path,
            key_path,
            keep_alive_interval_sec,
            // use_webtransport,
            log_level,
        }
    }
}

pub struct MOQTEndpoint {
    connection_creator: MOQTConnectionCreator,
}

impl MOQTEndpoint {
    pub fn create_client(port_num: u16) -> anyhow::Result<Self> {
        let client = QUICConnectionCreator::client(port_num)?;
        let creator = MOQTConnectionCreator::new(Box::new(client));
        Ok(Self {
            connection_creator: creator,
        })
    }

    pub fn create_server(
        cert_path: String,
        key_path: String,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        let server = QUICConnectionCreator::server(cert_path, key_path, port_num, keep_alive_sec)?;
        let creator = MOQTConnectionCreator::new(Box::new(server));
        Ok(Self {
            connection_creator: creator,
        })
    }

    pub async fn connect(
        &self,
        server_name: &str,
        port: u16,
    ) -> anyhow::Result<Arc<MOQTConnection>> {
        self.connection_creator
            .create_new_connection(server_name, port)
            .await
    }

    pub async fn accept(&mut self) -> anyhow::Result<Arc<MOQTConnection>> {
        self.connection_creator.accept_new_connection().await
    }
}
