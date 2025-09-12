use std::{fs, io, path::Path};

use moqt::{Endpoint, ServerConfig, QUIC};
use rcgen::{CertifiedKey, generate_simple_self_signed};

fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init()
        .ok();

    if Path::new("server-sample/keys/key.pem").exists()
        && Path::new("server-sample/keys/cert.pem").exists()
    {
        tracing::info!("Certificates already exist");
        Ok(())
    } else {
        let subject_alt_names = vec!["localhost".to_string()];
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(subject_alt_names).unwrap();
        let key_pem = signing_key.serialize_pem();
        fs::write("server-sample/keys/key.pem", key_pem)?;
        let cert_pem = cert.pem();
        fs::write("server-sample/keys/cert.pem", cert_pem)?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    create_certs_for_test_if_needed()?;
    let current_path = std::env::current_dir().expect("failed to get current path");
    let key_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/server-sample/keys/key.pem"
    );
    let cert_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/server-sample/keys/cert.pem"
    );
    tracing::info!("key_path: {}", key_path);
    tracing::info!("cert_path: {}", cert_path);
    
    let config = ServerConfig {
        port: 4433,
        key_path: key_path.clone(),
        cert_path: cert_path.clone(),
        keep_alive_interval_sec: 30
    };
    let mut endpoint = Endpoint::<QUIC>::create_server(config).unwrap();
    let connection = endpoint.accept().await;
    if let Err(e) = connection {
        panic!("test failed: {:?}", e)
    } else {
        println!("Please push the Enter key to continue...");
        let mut input = String::new();
        io::stdin().read_line(&mut input).expect("入力エラー");
        Ok(())
    }
}
