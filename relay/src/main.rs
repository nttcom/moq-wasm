use std::{
    fs,
    path::{Path, PathBuf},
};

use rcgen::{CertifiedKey, generate_simple_self_signed};
use tracing::Instrument;

const CERT_DIR: &str = "keys";

fn get_cert_path() -> PathBuf {
    let current = std::env::current_dir().unwrap();
    current.join(CERT_DIR).join("cert.pem")
}

fn get_key_path() -> PathBuf {
    let current = std::env::current_dir().unwrap();
    current.join(CERT_DIR).join("key.pem")
}

pub fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    unsafe { std::env::set_var("RUST_BACKTRACE", "full") };
    let current = std::env::current_dir()?;
    tracing::info!("current path: {}", current.to_str().unwrap());
    if !Path::new(CERT_DIR).exists() {
        fs::create_dir_all(CERT_DIR).unwrap();
    }

    if get_cert_path().exists() && get_key_path().exists() {
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
        fs::write(get_key_path(), key_pem)?;
        let cert_pem = cert.pem();
        fs::write(get_cert_path(), cert_pem)?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _logging = relay::init_logging("relay")?;
    let relay_span = tracing::info_span!(parent: None, "relay");

    async move {
        let startup_span = tracing::info_span!("startup");
        let (server, key_path, cert_path) = async move {
            create_certs_for_test_if_needed()?;

            let key_path = get_key_path().to_str().unwrap().to_string();
            let cert_path = get_cert_path().to_str().unwrap().to_string();

            let server = relay::RelayServer::new(&key_path, &cert_path);
            anyhow::Ok((server, key_path, cert_path))
        }
        .instrument(startup_span)
        .await?;

        let quic_handler = {
            let quic_listener_span = tracing::info_span!("quic_listener", port = 4434);
            async {
                tracing::info!(
                    key_path = %key_path,
                    cert_path = %cert_path,
                    "Starting QUIC listener"
                );
                server.spawn_transport::<moqt::QUIC>(4434)
            }
            .instrument(quic_listener_span)
            .await
        };

        let wt_handler = {
            let webtransport_listener_span =
                tracing::info_span!("webtransport_listener", port = 4433);
            async {
                tracing::info!(
                    key_path = %key_path,
                    cert_path = %cert_path,
                    "Starting WebTransport listener"
                );
                server.spawn_transport::<moqt::WEBTRANSPORT>(4433)
            }
            .instrument(webtransport_listener_span)
            .await
        };

        let (_server, _quic_handler, _wt_handler) = (server, quic_handler, wt_handler);

        tracing::info!("Relay server started with QUIC (4434) and WebTransport (4433)");
        tracing::info!("Ctrl+C to shutdown");

        let shutdown_span = tracing::info_span!("shutdown");
        async {
            tokio::signal::ctrl_c().await?;
            tracing::info!("Shutdown signal received. Closing...");
            tracing::info!("Relay server gracefully shutdown.");
            anyhow::Ok(())
        }
        .instrument(shutdown_span)
        .await?;

        Ok(())
    }
    .instrument(relay_span)
    .await
}
