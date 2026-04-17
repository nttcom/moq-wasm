mod logging;
pub mod modules; // modulesを公開

use modules::event_handler::EventHandler;
use modules::session_handler::SessionHandler;
use moqt::ServerConfig;
use std::sync::Arc;

use crate::modules::enums::MoqtRelayEvent;
use crate::modules::session_repository::SessionRepository;
pub use logging::{LoggingGuards, init_logging};
use tokio::sync::mpsc::UnboundedSender;

use tokio::sync::oneshot::Receiver; // Receiver for shutdown signal

pub struct RelayServer {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    sender: UnboundedSender<MoqtRelayEvent>,
    _manager: EventHandler,
    key_path: String,
    cert_path: String,
}

impl RelayServer {
    pub fn new(key_path: &str, cert_path: &str) -> Self {
        let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<MoqtRelayEvent>();

        // 共通のメッセージ処理ロジックを起動
        let manager = EventHandler::run(repo.clone(), receiver);

        Self {
            repo,
            sender,
            _manager: manager,
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

pub fn run_relay_server<T: moqt::TransportProtocol>(
    port: u16,
    shutdown_signal: Receiver<()>,
    key_path: &str,
    cert_path: &str,
) -> tokio::task::JoinHandle<()> {
    tracing::info!("key_path: {}", key_path);
    tracing::info!("cert_path: {}", cert_path);
    let key_path = key_path.to_string();
    let cert_path = cert_path.to_string();

    tokio::task::Builder::new()
        .name("RelayServer")
        .spawn(async move {
            tracing::info!("Relay server started");
            let server = RelayServer::new(&key_path, &cert_path);
            let _handler = server.spawn_transport::<T>(port);

            let _ = shutdown_signal.await.ok();
            tracing::info!("Relay server shutting down");
        })
        .unwrap()
}
