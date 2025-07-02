use std::sync::Arc;

use crate::modules::session_handlers::{
    quic_handler::QuicHandler, session_handler::SessionHandler,
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

pub struct MOQTClient {
    session_handler: Arc<Mutex<SessionHandler>>,
}

impl MOQTClient {
    pub fn new(config: &MOQTConfig) -> anyhow::Result<Self> {
        let handler = QuicHandler::new(
            config.cert_path.clone(),
            config.key_path.clone(),
            config.port,
            config.keep_alive_interval_sec,
        )
        .expect("failed to create MOQT client");
        let session_handler = Arc::new(Mutex::new(SessionHandler::new(Box::new(handler))));

        Ok(Self { session_handler })
    }
}
