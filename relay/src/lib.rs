pub mod modules; // modulesを公開

use std::sync::Arc;

use modules::event_handler::EventHandler;
use modules::relay::{
    cache::store::TrackCacheStore,
    caches::{
        delivery_type_map::DeliveryTypeMap,
        sender_map::SenderMap,
    },
    egress::coordinator::EgressCoordinator,
    ingest::stream_accepter::IngestCoordinator,
};
use modules::session_handler::SessionHandler;
use moqt::ServerConfig;

use crate::modules::enums::MOQTMessageReceived;
use crate::modules::session_repository::SessionRepository;

use console_subscriber::ConsoleLayer;
use tokio::sync::mpsc::UnboundedSender;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{self, EnvFilter, Layer, Registry, filter::LevelFilter, fmt};

pub fn init_logging(log_level: String) {
    let debug_console_filter =
        EnvFilter::new("tokio::task=trace,tokio::sync=trace,tokio::timer=trace");
    let debug_console_layer = ConsoleLayer::builder()
        .server_addr(([127, 0, 0, 1], 6669))
        .event_buffer_capacity(1024 * 250)
        .client_buffer_capacity(1024 * 7)
        .retention(std::time::Duration::from_secs(600))
        .spawn()
        .with_filter(debug_console_filter);

    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(log_level.parse().unwrap())
                .from_env_lossy(),
        );

    let file_layer = fmt::layer()
        .with_writer(rolling::hourly("./log", "output"))
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::DEBUG.into())
                .from_env_lossy(),
        );

    Registry::default()
        .with(debug_console_layer)
        .with(stdout_layer)
        .with(file_layer)
        .init();
}

use tokio::sync::oneshot::Receiver; // Receiver for shutdown signal

pub struct RelayServer {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    sender: UnboundedSender<MOQTMessageReceived>,
    ingest: IngestCoordinator,
    egress: EgressCoordinator,
    cache_store: Arc<TrackCacheStore>,
    manager: EventHandler,
    key_path: String,
    cert_path: String,
}

impl RelayServer {
    pub fn new(key_path: &str, cert_path: &str) -> Self {
        let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<MOQTMessageReceived>();

        let cache_store = Arc::new(TrackCacheStore::new());
        let sender_map = Arc::new(SenderMap::new());
        let delivery_type_map = Arc::new(DeliveryTypeMap::new());
        let ingest = IngestCoordinator::new(
            repo.clone(),
            cache_store.clone(),
            sender_map.clone(),
            delivery_type_map.clone(),
        );
        let egress = EgressCoordinator::new(
            repo.clone(),
            cache_store.clone(),
            sender_map,
            delivery_type_map,
        );

        // 共通のメッセージ処理ロジックを起動
        let manager = EventHandler::run(repo.clone(), receiver, ingest.sender(), egress.sender());

        Self {
            repo,
            sender,
            ingest,
            egress,
            cache_store,
            manager,
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

impl Drop for RelayServer {
    fn drop(&mut self) {
        tracing::trace!(
            manager_running = self.manager.is_running(),
            ingest_running = self.ingest.is_running(),
            ingest_tracks = self.ingest.track_count(),
            egress_running = self.egress.is_running(),
            cache_tracks = self.cache_store.len(),
            "relay runtime is kept alive until server drop"
        );
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
            let handler = server.spawn_transport::<T>(port);

            shutdown_signal.await.ok();
            drop(handler);
            tracing::info!("Relay server shutting down");
        })
        .unwrap()
}
