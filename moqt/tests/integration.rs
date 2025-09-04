use std::{fs, path::Path, thread::sleep, time::Duration};

use moqt::MOQTEndpoint;
use rcgen::{CertifiedKey, generate_simple_self_signed};

fn create_certs_for_test_if_needed() -> anyhow::Result<()> {
    unsafe { std::env::set_var("RUST_LOG", "trace") };
    if Path::new("key.pem").exists() && Path::new("cert.pem").exists() {
        tracing::info!("Certificates already exist");
        Ok(())
    } else {
        let subject_alt_names = vec!["localhost".to_string()];
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(subject_alt_names).unwrap();
        let key_pem = signing_key.serialize_pem();
        fs::write("key.pem", key_pem)?;
        let cert_pem = cert.pem();
        fs::write("cert.pem", cert_pem)?;

        Ok(())
    }
}

#[tokio::test]
async fn accept() -> anyhow::Result<()> {
    create_certs_for_test_if_needed()?;

    let current_path = std::env::current_dir().expect("failed to get current path");
    let key_path = format!("{}{}", current_path.to_str().unwrap(), "/key.pem");
    let cert_path = format!("{}{}", current_path.to_str().unwrap(), "/cert.pem");

    let mut endpoint = MOQTEndpoint::create_server(cert_path, key_path, 4433, 30).unwrap();
    let connection = endpoint.accept().await;
    match connection {
        Ok(_) => Ok(()),
        Err(e) => panic!("test failed: {:?}", e),
    }
}

#[tokio::test]
async fn connect() -> anyhow::Result<()> {
    create_certs_for_test_if_needed()?;

    let current_path = std::env::current_dir().expect("failed to get current path");
    let key_path = format!("{}{}", current_path.to_str().unwrap(), "/key.pem");
    let cert_path = format!("{}{}", current_path.to_str().unwrap(), "/cert.pem");

    let joinhandle = tokio::task::spawn(async {
        let mut endpoint = MOQTEndpoint::create_server(cert_path, key_path, 4433, 30).unwrap();
        let connection = endpoint.accept().await;
        assert!(connection.is_ok());
    });

    sleep(Duration::from_millis(100));

    let endpoint = MOQTEndpoint::create_client(0)?;
    let connection = endpoint.connect("localhost", 4433).await;
    assert!(connection.is_ok());

    let _ = joinhandle.await;
    Ok(())
}
