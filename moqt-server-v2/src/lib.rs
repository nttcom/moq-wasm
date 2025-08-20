use std::sync::Arc;

use crate::modules::session_handlers::{
    quic_handler::QuicConnectionCreator, underlay_protocol_handler::MOQTConnectionCreator,
};
use anyhow::Ok;
use tokio::sync::Mutex;

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

pub trait MOQTClientListener {
    fn on_connection_added();
    fn on_published();
    fn on_subscribed();
}

pub struct MOQTClient {
    underlay_handler: Arc<Mutex<MOQTConnectionCreator>>,
}

impl MOQTClient {
    pub fn new(config: &MOQTConfig) -> anyhow::Result<Self> {
        let handler = QuicConnectionCreator::new(
            config.cert_path.clone(),
            config.key_path.clone(),
            config.port,
            config.keep_alive_interval_sec,
        )
        .expect("failed to create MOQT client");
        let underlay_handler = Arc::new(Mutex::new(MOQTConnectionCreator::new(Box::new(handler))));

        Ok(Self { underlay_handler })
    }

    pub async fn connect(&self) {
        self.underlay_handler.lock().await.start();
    }

    pub async fn create_uni_stream(&self) {}

    pub async fn create_uni_datagram(&self) {}
}
