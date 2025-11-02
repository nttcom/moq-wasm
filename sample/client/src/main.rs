use std::time::Duration;
use tokio::time::sleep;

use crate::client::Client;

mod client;
mod stream_runner;

// room/user2 is notified from `Publish Namespace`.
fn create_client_thread(
    cert_path: String,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        let client = Client::new(cert_path, "user1".to_string()).await?;
        client.publish_namespace("room1/user1".to_string()).await;
        client.subscribe_namespace("room2".to_string()).await;
        client
            .publish("room1/user1".to_string(), "video".to_string())
            .await;
        // await until the application is shut down.
        let _ = signal_receiver.recv().await.ok();
        Ok(())
    })
}

// room/user1, room/user2 is notified from `Publish Namespace`.
fn create_client_thread2(
    cert_path: String,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        sleep(Duration::from_secs(5)).await;
        let client = Client::new(cert_path, "user2".to_string()).await?;
        client.publish_namespace("room2/user2".to_string()).await;
        client.subscribe_namespace("room1".to_string()).await;
        // client
        //     .publish("room2/user2".to_string(), "video".to_string())
        //     .await;
        // await until the application is shut down.
        let _ = signal_receiver.recv().await.ok();
        Ok(())
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    let current_path = std::env::current_dir().expect("failed to get current path");
    let cert_path = format!(
        "{}{}",
        current_path.to_str().unwrap(),
        "/sample/keys/cert.pem"
    );
    tracing::info!("cert_path: {}", cert_path);
    let mut thread_vec = vec![];
    let (signal_sender, signal_receiver) = tokio::sync::broadcast::channel::<()>(1);
    let thread = create_client_thread(cert_path.clone(), signal_receiver);
    let thread2 = create_client_thread2(cert_path, signal_sender.clone().subscribe());
    thread_vec.push(thread);
    thread_vec.push(thread2);

    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown");
    signal_sender.send(()).unwrap();
    thread_vec.iter().for_each(|t| {
        t.abort();
    });
    Ok(())
}
