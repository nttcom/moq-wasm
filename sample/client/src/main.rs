use anyhow::bail;
use moqt::{Endpoint, QUIC};
use std::{net::ToSocketAddrs, str::FromStr};

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
        "/sample/keys/cert.pem"
    );
    tracing::info!("cert_path: {}", cert_path);
    let (signal_sender, signal_receiver) = tokio::sync::oneshot::channel::<()>();

    let thread = tokio::task::Builder::new()
        .name("Client")
        .spawn(async move {
            let endpoint = Endpoint::<QUIC>::create_client_with_custom_cert(0, &cert_path)?;
            let url = url::Url::from_str("moqt://localhost:4433")?;
            let host = url.host_str().unwrap();
            let remote_address = (host, url.port().unwrap_or(4433))
                .to_socket_addrs()?
                .next()
                .unwrap();

            tracing::info!("remote_address: {} host: {}", remote_address, host);

            let session = match endpoint.connect(remote_address, host).await {
                Ok(s) => s,
                Err(e) => {
                    bail!("test failed: {:?}", e)
                },
            };
            tracing::info!("create session ok");
            let (publisher, subscriber) = session.create_publisher_subscriber_pair();
            let result = publisher.publish_namespace(vec!["test".to_string()]).await;
            if result.is_err() {
                tracing::info!("publish namespace error");
            } else {
                tracing::info!("publish namespace ok");
            }
            let result = subscriber
                .subscribe_namespace(vec!["sample".to_string()])
                .await;
            // await until the application is shut down.
            let _ = signal_receiver.await.ok();
            Ok(())
        })
        .unwrap();
    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown");
    signal_sender.send(()).unwrap();
    let _ = thread.await.unwrap();
    Ok(())
}
