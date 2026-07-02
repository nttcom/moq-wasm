use std::{str::FromStr, time::Duration};
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

use crate::client::Client;

mod client;
mod stream_runner;

#[derive(Clone, Copy, Debug)]
enum ManualClientTransport {
    Quic,
    WebTransport,
}

impl ManualClientTransport {
    fn from_env() -> anyhow::Result<Self> {
        match std::env::var("MOQT_TRANSPORT")
            .unwrap_or_else(|_| "quic".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "quic" | "moqt" => Ok(Self::Quic),
            "webtransport" | "web-transport" | "wt" | "https" => Ok(Self::WebTransport),
            value => anyhow::bail!("unsupported MOQT_TRANSPORT: {value}"),
        }
    }

    fn default_url(self) -> &'static str {
        match self {
            Self::Quic => "moqt://127.0.0.1:4433",
            Self::WebTransport => "https://127.0.0.1:4433",
        }
    }
}

// room/user2 is notified from `Publish Namespace`.
fn create_quic_client_thread(
    cert_path: String,
    moqt_url: String,
    verify_certificate: bool,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        let client =
            Client::<moqt::QUIC>::new(cert_path, moqt_url, verify_certificate, "user1".to_string())
                .await?;
        let _ = client.publish_namespace("room1/user1".to_string()).await;
        let _ = client.subscribe_namespace("room".to_string()).await;
        // client
        //     .publish("room1/user1".to_string(), "video".to_string())
        //     .await;
        // await until the application is shut down.
        let _ = signal_receiver.recv().await.ok();
        Ok(())
    })
}

// room/user1, room/user2 is notified from `Publish Namespace`.
fn create_quic_client_thread2(
    cert_path: String,
    moqt_url: String,
    verify_certificate: bool,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        sleep(Duration::from_secs(5)).await;
        let mut client =
            Client::<moqt::QUIC>::new(cert_path, moqt_url, verify_certificate, "user2".to_string())
                .await?;
        let _ = client.publish_namespace("room2/user2".to_string()).await;
        let _ = client.subscribe_namespace("room1".to_string()).await;
        client
            .active_subscribe(
                "user2".to_string(),
                "room1/user1".to_string(),
                "video".to_string(),
            )
            .await;

        // client
        //     .publish("room2/user2".to_string(), "video".to_string())
        //     .await;
        // await until the application is shut down.
        let _ = signal_receiver.recv().await.ok();
        Ok(())
    })
}

// room/user2 is notified from `Publish Namespace`.
fn create_webtransport_client_thread(
    cert_path: String,
    moqt_url: String,
    verify_certificate: bool,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        let client = Client::<moqt::WEBTRANSPORT>::new(
            cert_path,
            moqt_url,
            verify_certificate,
            "user1".to_string(),
        )
        .await?;
        let _ = client.publish_namespace("room1/user1".to_string()).await;
        let _ = client.subscribe_namespace("room".to_string()).await;
        let _ = signal_receiver.recv().await.ok();
        Ok(())
    })
}

// room/user1, room/user2 is notified from `Publish Namespace`.
fn create_webtransport_client_thread2(
    cert_path: String,
    moqt_url: String,
    verify_certificate: bool,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        sleep(Duration::from_secs(5)).await;
        let mut client = Client::<moqt::WEBTRANSPORT>::new(
            cert_path,
            moqt_url,
            verify_certificate,
            "user2".to_string(),
        )
        .await?;
        let _ = client.publish_namespace("room2/user2".to_string()).await;
        let _ = client.subscribe_namespace("room1".to_string()).await;
        client
            .active_subscribe(
                "user2".to_string(),
                "room1/user1".to_string(),
                "video".to_string(),
            )
            .await;
        let _ = signal_receiver.recv().await.ok();
        Ok(())
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::from_str("manual_client=debug,moqt=debug").unwrap()),
        )
        .with_line_number(true)
        .try_init()
        .ok();

    let current_path = std::env::current_dir().expect("failed to get current path");
    let cert_path = format!("{}{}", current_path.to_str().unwrap(), "/keys/cert.pem");
    let transport = ManualClientTransport::from_env()?;
    let moqt_url =
        std::env::var("MOQT_URL").unwrap_or_else(|_| transport.default_url().to_string());
    let verify_certificate = !env_flag("MOQT_INSECURE_SKIP_TLS_VERIFY");
    tracing::info!("cert_path: {}", cert_path);
    tracing::info!("transport: {:?}", transport);
    tracing::info!("moqt_url: {}", moqt_url);
    tracing::info!("verify_certificate: {}", verify_certificate);
    let mut thread_vec = vec![];
    let (signal_sender, signal_receiver) = tokio::sync::broadcast::channel::<()>(1);
    let (thread, thread2) = match transport {
        ManualClientTransport::Quic => (
            create_quic_client_thread(
                cert_path.clone(),
                moqt_url.clone(),
                verify_certificate,
                signal_receiver,
            ),
            create_quic_client_thread2(
                cert_path,
                moqt_url,
                verify_certificate,
                signal_sender.clone().subscribe(),
            ),
        ),
        ManualClientTransport::WebTransport => (
            create_webtransport_client_thread(
                cert_path.clone(),
                moqt_url.clone(),
                verify_certificate,
                signal_receiver,
            ),
            create_webtransport_client_thread2(
                cert_path,
                moqt_url,
                verify_certificate,
                signal_sender.clone().subscribe(),
            ),
        ),
    };
    thread_vec.push(thread);
    thread_vec.push(thread2);

    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown");
    let _ = signal_sender.send(());
    thread_vec.iter().for_each(|t| {
        t.abort();
    });
    Ok(())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
