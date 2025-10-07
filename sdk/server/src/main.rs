mod modules;

use std::{fs, path::Path};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use modules::handler::Handler;
use modules::manager::Manager;
use uuid::Uuid;
use modules::core::{publisher::Publisher, session::Session, subscriber::Subscriber};

fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init()
        .ok();
    let current = std::env::current_dir()?;
    tracing::info!("current path: {}", current.to_str().unwrap());

    if Path::new("sample/server-sample/keys/key.pem").exists()
        && Path::new("sample/server-sample/keys/cert.pem").exists()
    {
        tracing::info!("Certificates already exist");
        Ok(())
    } else {
        let subject_alt_names = vec!["localhost".to_string()];
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(subject_alt_names).unwrap();
        let key_pem = signing_key.serialize_pem();
        fs::write("sample/server-sample/keys/key.pem", key_pem)?;
        let cert_pem = cert.pem();
        fs::write("sample/server-sample/keys/cert.pem", cert_pem)?;

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
        "/sample/server/keys/key.pem"
    );
    let cert_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/sample/server/keys/cert.pem"
    );
    tracing::info!("key_path: {}", key_path);
    tracing::info!("cert_path: {}", cert_path);
    
    tokio::spawn(async move {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<(
            Uuid,
            Box<dyn Session>,
            Box<dyn Publisher>,
            Box<dyn Subscriber>,
        )>();
        let _handler = Handler::run(key_path, cert_path, sender);
        let _manager = Manager::run(receiver);
    });
    Ok(())
}
