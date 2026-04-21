use std::sync::Arc;

use moqt::ServerConfig;
use tokio::sync::mpsc::UnboundedSender;

use crate::modules::{
    enums::MoqtRelayEvent, session_handler::SessionHandler,
    session_repository::SessionRepository,
};
use crate::relay_server::{runtime::RelayRuntime, store::RelayStore};

pub struct RelayServer {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    sender: UnboundedSender<MoqtRelayEvent>,
    _store: Arc<RelayStore>,
    _runtime: RelayRuntime,
    key_path: String,
    cert_path: String,
}

impl RelayServer {
    pub fn new(key_path: &str, cert_path: &str) -> Self {
        let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let store = RelayStore::new();
        let (sender, runtime) = RelayRuntime::new(repo.clone(), &store);

        Self {
            repo,
            sender,
            _store: store,
            _runtime: runtime,
            key_path: key_path.to_string(),
            cert_path: cert_path.to_string(),
        }
    }

    pub fn spawn_transport<T: moqt::TransportProtocol>(&self, port: u16) -> SessionHandler {
        tracing::info!("Spawning transport handler on port {}", port);
        let server_config = ServerConfig {
            port,
            cert_path: self.cert_path.clone(),
            key_path: self.key_path.clone(),
            keep_alive_interval_sec: 15,
        };

        SessionHandler::run::<T>(server_config, self.repo.clone(), self.sender.clone())
    }
}
