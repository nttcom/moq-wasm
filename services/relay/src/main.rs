use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::Context;
use moqt::server::write_self_signed_cert;
use relay::ProcessGauges;
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

fn create_certs_for_test_if_needed(cert_dir: &Path) -> anyhow::Result<()> {
    tracing::debug!(cert_dir = %cert_dir.display(), "relay certificate directory");
    if has_cert_pair(cert_dir) {
        tracing::debug!("certificates already exist");
        return Ok(());
    }
    // Relays started together race to create `cert_dir`; the rename keeps it
    // to one complete pair.
    static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);
    let mut staging_name = cert_dir.file_name().unwrap_or_default().to_os_string();
    staging_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
    ));
    let staging_dir = cert_dir.with_file_name(staging_name);
    write_self_signed_cert(
        &staging_dir,
        &["localhost", "127.0.0.1", "relay-a", "relay-b"],
    )?;
    if let Err(error) = std::fs::rename(&staging_dir, cert_dir) {
        std::fs::remove_dir_all(&staging_dir)?;
        if !has_cert_pair(cert_dir) {
            return Err(error).context("failed to move the certificates into place");
        }
        tracing::debug!("another relay wrote the certificates first");
    }
    Ok(())
}

fn has_cert_pair(cert_dir: &Path) -> bool {
    cert_dir.join("cert.pem").exists() && cert_dir.join("key.pem").exists()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    unsafe { std::env::set_var("RUST_BACKTRACE", "full") };
    let logging = relay::init_logging("relay")?;
    let cert_dir = cert_dir();
    create_certs_for_test_if_needed(&cert_dir)?;
    let config = relay::RelayConfig::from_env()?;

    let key_path = cert_dir.join("key.pem").to_str().unwrap().to_string();
    let cert_path = cert_dir.join("cert.pem").to_str().unwrap().to_string();

    let server = relay::RelayServer::new_with_config(&key_path, &cert_path, config.clone()).await?;
    let mut client_handler = server.spawn_client_transport::<moqt::NATIVE>(config.port)?;
    let _inner_handler = server.spawn_inner_transport::<moqt::QUIC>(config.inner_port)?;
    let _stats_exporter = logging
        .event_logger("relay")
        .zip(logging.meter("relay"))
        .map(|(event_logger, meter)| {
            server.spawn_stats_exporter(
                config.relay_id.clone(),
                event_logger,
                ProcessGauges::new(&meter),
            )
        });

    tracing::info!(
        port = config.port,
        inner_port = config.inner_port,
        "Relay server started with external QUIC + WebTransport and inner QUIC"
    );
    tracing::debug!("Ctrl+C or SIGTERM to shut down");

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

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Barrier},
        thread,
    };

    use super::create_certs_for_test_if_needed;

    fn read_pair(dir: &Path) -> (String, String) {
        (
            std::fs::read_to_string(dir.join("cert.pem")).unwrap(),
            std::fs::read_to_string(dir.join("key.pem")).unwrap(),
        )
    }

    #[test]
    fn relays_starting_together_read_one_certificate_pair() {
        // Arrange
        let parent = std::env::temp_dir().join(format!("relay-main-certs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let cert_dir = parent.join("keys");
        let start = Arc::new(Barrier::new(16));

        // Act
        let pairs = (0..16)
            .map(|_| {
                let cert_dir = cert_dir.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    create_certs_for_test_if_needed(&cert_dir).unwrap();
                    read_pair(&cert_dir)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|relay| relay.join().unwrap())
            .collect::<Vec<_>>();

        // Assert
        let on_disk = read_pair(&cert_dir);
        assert!(pairs.iter().all(|pair| *pair == on_disk));
        std::fs::remove_dir_all(&parent).unwrap();
    }
}
