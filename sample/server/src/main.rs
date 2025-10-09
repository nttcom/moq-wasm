mod modules;

use modules::core::{publisher::Publisher, session::Session, subscriber::Subscriber};
use modules::handler::Handler;
use modules::manager::Manager;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use std::{fs, path::Path};

use crate::modules::types::SessionId;

fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init()
        .ok();
    let current = std::env::current_dir()?;
    tracing::info!("current path: {}", current.to_str().unwrap());

    if Path::new("sample/keys/key.pem").exists() && Path::new("sample/keys/cert.pem").exists() {
        tracing::info!("Certificates already exist");
        Ok(())
    } else {
        let subject_alt_names = vec!["localhost".to_string()];
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
    create_certs_for_test_if_needed()?;
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
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<(
                SessionId,
                Box<dyn Session>,
                Box<dyn Publisher>,
                Box<dyn Subscriber>,
            )>();
            let _handler = Handler::run(key_path, cert_path, sender);
            let _manager = Manager::run(receiver);
            // await until the application is shut down.
            let _ = signal_receiver.await.ok();
        }).unwrap();
    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await.unwrap();
    tracing::info!("shutdown");
    signal_sender.send(()).unwrap();
    thread.await.unwrap();
    Ok(())
}
