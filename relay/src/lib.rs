pub mod modules; // modulesを公開

use modules::event_handler::EventHandler;
use modules::session_handler::SessionHandler;
use moqt::ServerConfig;
use std::sync::Arc;

use crate::modules::enums::MOQTMessageReceived;
use crate::modules::repositories::session_repository::SessionRepository;

use console_subscriber::ConsoleLayer;
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

pub fn run_relay_server(
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
        .name("Handler")
        .spawn(async move {
            tracing::info!("Handler started");
            let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<MOQTMessageReceived>();

            // ServerConfigのポートを動的に設定
            let server_config = ServerConfig {
                port, // 引数で渡されたポートを使用
                cert_path: cert_path.clone(),
                key_path: key_path.clone(),
                keep_alive_interval_sec: 15,
            };

            let _handler = SessionHandler::run(server_config, repo.clone(), sender); // SessionHandler::runのシグネチャも変更する
            let _manager = EventHandler::run(repo, receiver);
            let _ = shutdown_signal.await.ok(); // 関数の引数として受け取ったshutdown_signalを使用
        })
        .unwrap()
}
