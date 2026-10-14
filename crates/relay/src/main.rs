use std::{path::PathBuf, time::Duration};

use moqt::server::write_self_signed_cert;
use tokio::signal::unix::{SignalKind, signal};

const CERT_DIR: &str = "keys";
/// Bound on waiting for the CONNECTION_CLOSE of the sessions closed with
/// GOAWAY_TIMEOUT to leave the socket before the process exits.
const CLOSE_FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

/// `docker stop` and Kubernetes stop the relay with SIGTERM, not SIGINT.
async fn shutdown_signal() -> anyhow::Result<()> {
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {}
    }
    Ok(())
}

fn cert_dir() -> PathBuf {
    std::env::var_os("RELAY_CERT_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(CERT_DIR))
}

fn get_cert_path() -> PathBuf {
    cert_dir().join("cert.pem")
}

fn get_key_path() -> PathBuf {
    cert_dir().join("key.pem")
}

pub fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    unsafe { std::env::set_var("RUST_BACKTRACE", "full") };
    let cert_dir = cert_dir();
    tracing::info!("relay cert dir: {}", cert_dir.to_string_lossy());
    if get_cert_path().exists() && get_key_path().exists() {
        tracing::info!("Certificates already exist");
    } else {
        write_self_signed_cert(&cert_dir, &["localhost", "127.0.0.1", "relay-a", "relay-b"])?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _logging = relay::init_logging("relay")?;
    create_certs_for_test_if_needed()?;
    let config = relay::RelayConfig::from_env()?;

    let key_path = get_key_path().to_str().unwrap().to_string();
    let cert_path = get_cert_path().to_str().unwrap().to_string();

    let server = relay::RelayServer::new_with_config(&key_path, &cert_path, config.clone()).await?;
    let mut client_handler = server.spawn_client_transport::<moqt::DUAL>(config.port)?;
    let _inner_handler = server.spawn_inner_transport::<moqt::QUIC>(config.inner_port)?;
    let _stats_publisher = server.spawn_stats_publisher(config.relay_id.clone(), config.inner_port);

    tracing::info!(
        port = config.port,
        inner_port = config.inner_port,
        "Relay server started with external QUIC + WebTransport and inner QUIC"
    );
    tracing::info!("Ctrl+C or SIGTERM to shutdown");

    shutdown_signal().await?;
    tracing::info!(
        drain_timeout_secs = config.drain_timeout.as_secs(),
        "Shutdown signal received; draining client sessions (signal again to exit at once)"
    );
    client_handler.stop_accepting();
    tokio::select! {
        _ = server.drain(config.drain_timeout) => {
            let _ = tokio::time::timeout(CLOSE_FLUSH_TIMEOUT, client_handler.wait_idle()).await;
            tracing::info!("Relay server gracefully shutdown.");
        }
        _ = shutdown_signal() => {
            tracing::warn!("Second shutdown signal received; exiting without waiting for the clients");
        }
    }
    Ok(())
}
