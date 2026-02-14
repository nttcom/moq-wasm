use std::{
    fs,
    path::{Path, PathBuf},
};

use rcgen::{CertifiedKey, generate_simple_self_signed};
use relay::run_relay_server;
use tokio::sync::oneshot;

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
    // ロギングの初期化 (必要であれば)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    let (tx, rx) = oneshot::channel();
    create_certs_for_test_if_needed()?;

    // run_relay_serverをバックグラウンドで実行
    let relay_handle = run_relay_server(
        4434,
        rx,
        get_key_path().to_str().unwrap(),
        get_cert_path().to_str().unwrap(),
    );
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
