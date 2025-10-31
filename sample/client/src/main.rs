use anyhow::bail;
use moqt::{Endpoint, QUIC};
use std::{
    net::ToSocketAddrs,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::time::sleep;

static TRACK_ALIAS_1: AtomicU64 = AtomicU64::new(0);
static TRACK_ALIAS_2: AtomicU64 = AtomicU64::new(0);

// room/user2 is notified from `Publish Namespace`.
fn create_client_thread(
    cert_path: String,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
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
            }
        };
        let session = Arc::new(session);
        let th = create_receive_thread("thread_1".to_string(), session.clone());
        tracing::info!("create session ok");
        let (publisher, subscriber) = session.create_publisher_subscriber_pair();
        let result = publisher.publish_namespace("room/user1".to_string()).await;
        if result.is_err() {
            tracing::info!("publish namespace error");
        } else {
            tracing::info!("publish namespace ok");
        }
        let result = subscriber
            .subscribe_namespace("room/user2".to_string())
            .await;
        if result.is_err() {
            tracing::info!("subscribe namespace error");
        } else {
            tracing::info!("subscribe namespace ok");
        }
        let option = moqt::PublishOption::default();
        TRACK_ALIAS_1.fetch_add(option.track_alias, Ordering::SeqCst);
        let pub_result = publisher
            .publish("room/user1".to_string(), "video".to_string(), option)
            .await;
        if pub_result.is_err() {
            tracing::info!("publish error");
        } else {
            tracing::info!("publish ok");
        }
        // await until the application is shut down.
        let _ = signal_receiver.recv().await.ok();
        th.abort();
        Ok(())
    })
}

fn create_receive_thread(
    label: String,
    session: Arc<moqt::Session<QUIC>>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        loop {
            let result = session.receive_event().await;
            if let Err(e) = result {
                tracing::error!("Failed to receive event: {}", e);
                break;
            }
            let event = result.unwrap();
            match event {
                moqt::SessionEvent::PublishNamespace(publish_namespace_handler) => {
                    tracing::info!(
                        "Received: {} Publish Namespace: {}",
                        label,
                        publish_namespace_handler.track_namespace
                    );
                    let _ = publish_namespace_handler.ok().await;
                }
                moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                    tracing::info!(
                        "Received: {} Subscribe Namespace: {}",
                        label,
                        subscribe_namespace_handler.track_namespace_prefix
                    );
                    let _ = subscribe_namespace_handler.ok().await;
                }
                moqt::SessionEvent::Publish(publish_handler) => {
                    tracing::info!("Received: {} Publish", label);
                    let _ = publish_handler
                        .ok(128, moqt::FilterType::LatestObject)
                        .await;
                }
                moqt::SessionEvent::Subscribe(subscribe_handler) => {
                    tracing::info!("Received: {} Subscribe", label);
                    let track_alias = TRACK_ALIAS_2.fetch_add(1, Ordering::SeqCst);
                    let _ = subscribe_handler.ok(track_alias, 1000000, false).await;
                }
                moqt::SessionEvent::ProtocolViolation() => {
                    tracing::info!("Received: {} ProtocolViolation", label);
                }
            };
        }
    })
}

// room/user1, room/user2 is notified from `Publish Namespace`.
fn create_client_thread2(
    cert_path: String,
    mut signal_receiver: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        sleep(Duration::from_secs(3)).await;
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
            }
        };
        let session = Arc::new(session);
        let th = create_receive_thread2("thread_2".to_string(), session.clone());
        tracing::info!("create session ok");
        let (publisher, subscriber) = session.create_publisher_subscriber_pair();
        let result = publisher.publish_namespace("room/user2".to_string()).await;
        if result.is_err() {
            tracing::info!("publish namespace error");
        } else {
            tracing::info!("publish namespace ok");
        }
        let result = subscriber.subscribe_namespace("room".to_string()).await;
        // await until the application is shut down.
        let _ = signal_receiver.recv().await.ok();
        th.abort();
        Ok(())
    })
}

fn create_receive_thread2(
    label: String,
    session: Arc<moqt::Session<QUIC>>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        loop {
            let result = session.receive_event().await;
            if let Err(e) = result {
                tracing::error!("Failed to receive event: {}", e);
                break;
            }
            let event = result.unwrap();
            match event {
                moqt::SessionEvent::PublishNamespace(publish_namespace_handler) => {
                    tracing::info!(
                        "Received: {} Publish Namespace: {}",
                        label,
                        publish_namespace_handler.track_namespace
                    );
                    let _ = publish_namespace_handler.ok().await;
                }
                moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                    tracing::info!(
                        "Received: {} Subscribe Namespace: {}",
                        label,
                        subscribe_namespace_handler.track_namespace_prefix
                    );
                    let _ = subscribe_namespace_handler.ok().await;
                }
                moqt::SessionEvent::Publish(publish_handler) => {
                    tracing::info!("Received: {} Publish", label);
                    let pub_handler_result = publish_handler
                        .ok(128, moqt::FilterType::LatestObject)
                        .await;
                    if pub_handler_result.is_err() {
                        tracing::error!("{} :publish error", label);
                    } else {
                        tracing::info!("{} :publish ok", label);
                        let option = moqt::SubscribeOption {
                            subscriber_priority: 128,
                            group_order: publish_handler.group_order,
                            forward: publish_handler.forward,
                            filter_type: moqt::FilterType::LatestObject,
                            start_location: None,
                            end_group: None,
                        };
                        let subscription = publish_handler
                            .subscribe(
                                publish_handler.track_namespace.to_string(),
                                publish_handler.track_name.to_string(),
                                option,
                            )
                            .await;
                        if subscription.is_err() {
                            tracing::error!("{} :subscribe error", label);
                        } else {
                            tracing::info!("{} :subscribe ok", label);
                        }
                    }
                }
                moqt::SessionEvent::Subscribe(subscribe_handler) => {
                    tracing::info!("Received: {} Subscribe", label);
                    let track_alias = TRACK_ALIAS_1.fetch_add(1, Ordering::SeqCst);
                    let _ = subscribe_handler.ok(track_alias, 1000000, false).await;
                }
                moqt::SessionEvent::ProtocolViolation() => {
                    tracing::info!("Received: {} ProtocolViolation", label);
                }
            };
        }
    })
}

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
