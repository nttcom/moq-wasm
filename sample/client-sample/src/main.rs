use std::{net::ToSocketAddrs, str::FromStr};

use moqt::{Endpoint, QUIC};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init()
        .ok();

    let current_path = std::env::current_dir().expect("failed to get current path");
    let cert_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/sample/client-sample/keys/cert.pem"
    );
    tracing::info!("cert_path: {}", cert_path);

    let endpoint = Endpoint::<QUIC>::create_client_with_custom_cert(0, &cert_path)?;
    let url = url::Url::from_str("moqt://localhost:4433")?;
    let host = url.host_str().unwrap();
    let remote_address = (host, url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .unwrap();

    tracing::info!("remote_address: {} host: {}", remote_address, host);

    let connection = endpoint.connect(remote_address, host).await;
    if let Err(e) = connection {
        panic!("test failed: {:?}", e)
    } else {
        println!("Please input `Ctrl + C` to finish...");
        tokio::signal::ctrl_c().await?;
        Ok(())
    }
}
