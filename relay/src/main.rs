use std::{
    env, fs,
    path::{Path, PathBuf},
};

use rcgen::{CertifiedKey, generate_simple_self_signed};
use relay::run_relay_server;
use tokio::sync::oneshot;

const CERT_DIR: &str = "keys";

#[derive(Clone, Copy, Debug)]
enum TransportKind {
    Quic,
    WebTransport,
}

#[derive(Debug)]
struct RelayCli {
    transport: TransportKind,
    port: u16,
}

impl Default for RelayCli {
    fn default() -> Self {
        Self {
            transport: TransportKind::Quic,
            port: 4434,
        }
    }
}

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
            "127.0.0.1".to_string(),
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

fn print_usage() {
    println!(
        "Usage: cargo run -p relay -- [--transport quic|webtransport] [--port <PORT>]\n\
         Defaults: --transport quic --port 4434"
    );
}

fn parse_args() -> anyhow::Result<RelayCli> {
    let mut cli = RelayCli::default();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--transport" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--transport requires a value"))?;
                cli.transport = match value.as_str() {
                    "quic" => TransportKind::Quic,
                    "webtransport" => TransportKind::WebTransport,
                    _ => {
                        return Err(anyhow::anyhow!(
                            "unsupported transport: {value}. expected quic or webtransport"
                        ));
                    }
                };
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--port requires a value"))?;
                cli.port = value.parse::<u16>()?;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {
                return Err(anyhow::anyhow!("unknown argument: {arg}"));
            }
        }
    }

    Ok(cli)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ロギングの初期化 (必要であれば)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    let cli = parse_args()?;
    let (tx, rx) = oneshot::channel();
    create_certs_for_test_if_needed()?;

    let relay_handle = match cli.transport {
        TransportKind::Quic => {
            tracing::info!("starting relay with QUIC on port {}", cli.port);
            run_relay_server::<moqt::QUIC>(
                cli.port,
                rx,
                get_key_path().to_str().unwrap(),
                get_cert_path().to_str().unwrap(),
            )
        }
        TransportKind::WebTransport => {
            tracing::info!("starting relay with WebTransport on port {}", cli.port);
            run_relay_server::<moqt::WEBTRANSPORT>(
                cli.port,
                rx,
                get_key_path().to_str().unwrap(),
                get_cert_path().to_str().unwrap(),
            )
        }
    };
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown signal sent");
    let _ = tx.send(()); // シャットダウンシグナルを送信

    // relay_handleの終了を待つ
    relay_handle.await?; // エラーを伝播

    tracing::info!("Relay server gracefully shutdown.");
    Ok(())
}
