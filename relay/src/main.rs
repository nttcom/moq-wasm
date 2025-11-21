mod modules;

use modules::event_handler::EventHandler;
use modules::session_handler::SessionHandler;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use std::sync::Arc;
use std::{fs, path::Path};

use crate::modules::enums::MOQTMessageReceived;
use crate::modules::repositories::session_repository::SessionRepository;

use console_subscriber::ConsoleLayer;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{self, EnvFilter, Layer, Registry, filter::LevelFilter, fmt};
pub fn init_logging(log_level: String) {
    // tokio-console用のレイヤーとフィルタ(For Development)
    // let console_filter = EnvFilter::new("tokio::task=trace");
    // let console_layer = ConsoleLayer::builder()
    //     .retention(std::time::Duration::from_secs(3600)) // Default: 3600
    //     .spawn()
    //     .with_filter(console_filter);
    // tokio-console用のレイヤーとフィルタ(For Debug)
    let debug_console_filter =
        EnvFilter::new("tokio::task=trace,tokio::sync=trace,tokio::timer=trace");
    let debug_console_layer = ConsoleLayer::builder()
        .server_addr(([127, 0, 0, 1], 6669))
        .event_buffer_capacity(1024 * 250) // Default: 102400
        .client_buffer_capacity(1024 * 7) // Default: 1024
        .retention(std::time::Duration::from_secs(600)) // Default: 3600
        .spawn()
        .with_filter(debug_console_filter);

    // 標準出力用のレイヤーとフィルタ
    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(log_level.parse().unwrap())
                .from_env_lossy(),
        );

    // ログファイル用のレイヤーとフィルタ
    let file_layer = fmt::layer()
        .with_writer(rolling::hourly("./log", "output"))
        // Multi Writer with_ansi option doesn't work https://github.com/tokio-rs/tracing/issues/3116
        // .with_ansi(false)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::DEBUG.into())
                .from_env_lossy(),
        );

    Registry::default()
        // .with(console_layer)
        .with(debug_console_layer)
        .with(stdout_layer)
        .with(file_layer)
        .init();
}

fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    unsafe { std::env::set_var("RUST_BACKTRACE", "full") };
    // init_logging("INFO".to_string());
    let current = std::env::current_dir()?;
    tracing::info!("current path: {}", current.to_str().unwrap());

    if Path::new("sample/keys/key.pem").exists() && Path::new("sample/keys/cert.pem").exists() {
        tracing::info!("Certificates already exist");
        Ok(())
    } else {
        let subject_alt_names = vec![
            "localhost".to_string(),
            "moqt.research.skyway.io".to_string(),
        ];
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(subject_alt_names).unwrap();
        let key_pem = signing_key.serialize_pem();
        fs::write("sample/keys/key.pem", key_pem)?;
        let cert_pem = cert.pem();
        fs::write("sample/keys/cert.pem", cert_pem)?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();
    create_certs_for_test_if_needed()?;
    // console_subscriber::init();
    let current_path = std::env::current_dir().expect("failed to get current path");
    let key_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/sample/keys/key.pem"
    );
    let cert_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/sample/keys/cert.pem"
    );
    tracing::info!("key_path: {}", key_path);
    tracing::info!("cert_path: {}", cert_path);
    let (signal_sender, signal_receiver) = tokio::sync::oneshot::channel::<()>();

    let thread = tokio::task::Builder::new()
        .name("Handler")
        .spawn(async move {
            tracing::info!("Handler started");
            let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<MOQTMessageReceived>();
            let _handler = SessionHandler::run(key_path, cert_path, repo.clone(), sender);
            let _manager = EventHandler::run(repo, receiver);
            // await until the application is shut down.
            let _ = signal_receiver.await.ok();
        })
        .unwrap();
    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await.unwrap();
    tracing::info!("shutdown");
    signal_sender.send(()).unwrap();
    thread.await.unwrap();
    Ok(())
}
