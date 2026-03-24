use std::net::ToSocketAddrs;
use std::str::FromStr;

use anyhow::{Context as _, Result};
use moqt::{ClientConfig, ContentExists, Endpoint, QUIC, SessionEvent, StreamDataSender};
use tokio::sync::oneshot;
use tracing::info;

/// MoQT に接続し、namespace を publish し、StreamDataSender を返す。
/// subscriber が来るまでイベントループで待機する。
pub async fn connect_and_wait_for_subscriber(namespace: &str) -> Result<StreamDataSender<QUIC>> {
    let config = ClientConfig {
        port: 0,
        verify_certificate: false,
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    let url = url::Url::from_str("moqt://localhost:4434")?;
    let host = url.host_str().unwrap();
    let remote_address = (host, url.port().unwrap())
        .to_socket_addrs()?
        .next()
        .context("failed to resolve address")?;

    info!(%remote_address, "connecting to relay");
    let session = endpoint.connect(remote_address, host).await?;
    let (publisher, _subscriber) = session.publisher_subscriber_pair();
    let publisher = std::sync::Arc::new(publisher);

    publisher
        .publish_namespace(namespace.to_string())
        .await
        .context("failed to publish namespace")?;
    info!(namespace, "namespace published");

    let (tx, rx) = oneshot::channel::<StreamDataSender<QUIC>>();
    let pub_clone = publisher.clone();

    // イベント処理タスク
    tokio::spawn(async move {
        let mut tx = Some(tx);
        loop {
            let event = match session.receive_event().await {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("event loop error: {}", e);
                    break;
                }
            };
            match event {
                SessionEvent::Subscribe(handler) => {
                    info!(
                        namespace = handler.track_namespace,
                        track = handler.track_name,
                        "received subscribe"
                    );
                    let Ok(_track_alias) = handler.ok(1_000_000, ContentExists::False).await else {
                        tracing::error!("failed to send subscribe ok");
                        continue;
                    };
                    let publication = handler.into_publication(0);
                    let stream = match pub_clone.create_stream(&publication).await {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("failed to create stream: {}", e);
                            continue;
                        }
                    };
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(stream);
                    }
                }
                SessionEvent::ProtocolViolation() => {
                    tracing::error!("protocol violation");
                    break;
                }
                _ => {}
            }
        }
    });

    info!("waiting for subscriber...");
    let stream = rx
        .await
        .context("event task dropped before subscriber arrived")?;
    info!("subscriber connected, stream ready");
    Ok(stream)
}
