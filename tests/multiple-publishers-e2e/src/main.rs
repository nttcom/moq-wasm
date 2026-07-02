//! Manual e2e for the single-writer (first-writer-wins) guard.
//!
//! Alice publishes a track and Bob subscribes to it (data flows). Carol then
//! publishes the *same* Full Track Name. The relay keeps Alice as the writer and
//! ignores Carol, so Bob must only ever receive Alice's objects. Payloads are
//! tagged with the publisher name so Bob can tell them apart.
//!
//! Carol also disconnects after publishing. Because Carol shares Alice's Full
//! Track Name, a buggy relay would tear down Alice's ingest on Carol's cleanup;
//! Bob must keep receiving Alice's later groups regardless.
//!
//! Run a relay on localhost:4433, then `cargo run -p multiple-publishers-e2e`
//! from the repo root. Bob asserts `carol == 0`, that Alice keeps flowing after
//! Carol leaves, and prints a summary.

use std::env;
use std::net::ToSocketAddrs;
use std::str::FromStr;
use std::time::Duration;

use moqt::{
    ClientConfig, DataReceiver, Endpoint, ExtensionHeaders, FilterType, GroupOrder, PublishOption,
    QUIC, Session, StreamDataReceiverFactory, StreamDataSenderFactory, Subgroup, SubgroupId,
    SubgroupObject, SubscribeOption,
};
use tokio::sync::oneshot;

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const NAMESPACE: &str = "room/main";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const ALICE_GROUPS: u64 = 8;
const CAROL_GROUPS: u64 = 4;
const OBJECTS_PER_GROUP: u64 = 5;
// Alice sends this group only after Carol has joined and left, so receiving it
// proves Carol's departure did not stop Alice's ingest.
const LATE_GROUP: u64 = 4;

async fn new_session() -> anyhow::Result<Session<QUIC>> {
    let relay_url =
        env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    let url = url::Url::from_str(&relay_url).unwrap();
    let host = url.host_str().unwrap();
    let remote = (host, url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .unwrap();
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
    })?;
    let connecting = endpoint.connect(remote, host).await?;
    connecting.await
}

async fn send_group(
    factory: &StreamDataSenderFactory<QUIC>,
    who: &str,
    group_id: u64,
) -> anyhow::Result<()> {
    let sender = factory.next().await?;
    let header = sender.create_header(group_id, SubgroupId::None, PUBLISHER_PRIORITY, false, false);
    let mut stream = sender.send_header(header).await?;
    for obj_id in 0..OBJECTS_PER_GROUP {
        let payload = format!("{}:g{}:o{}", who, group_id, obj_id);
        let obj = stream.create_object_field(
            0,
            ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            SubgroupObject::new_payload(payload.into()),
        );
        stream.send(obj).await?;
        tracing::info!("[{}] sent g{}:o{}", who, group_id, obj_id);
    }
    stream.close().await
}

async fn alice(
    ready_tx: oneshot::Sender<()>,
    done_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let session = new_session().await?;
    let publisher = session.publisher();
    let subscription = publisher
        .publish(
            NAMESPACE.to_string(),
            TRACK_NAME.to_string(),
            PublishOption::default(),
        )
        .await?;
    tracing::info!("[alice] publish ok");
    let factory = publisher.create_stream(&subscription);
    let _ = ready_tx.send(());

    for group_id in 0..ALICE_GROUPS {
        send_group(&factory, "alice", group_id).await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    tracing::info!("[alice] all groups sent");
    let _ = done_rx.await;
    Ok(())
}

async fn carol(go_rx: oneshot::Receiver<()>) -> anyhow::Result<()> {
    let _ = go_rx.await;
    // Give Alice's ingress a moment to be the established writer before joining.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let session = new_session().await?;
    let publisher = session.publisher();
    let subscription = match publisher
        .publish(
            NAMESPACE.to_string(),
            TRACK_NAME.to_string(),
            PublishOption::default(),
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::info!("[carol] publish rejected by relay: {}", e);
            return Ok(());
        }
    };
    tracing::info!("[carol] publish ok; relay should ignore this writer (first-writer-wins)");
    let factory = publisher.create_stream(&subscription);
    for group_id in 0..CAROL_GROUPS {
        send_group(&factory, "carol", group_id).await?;
    }
    tracing::info!("[carol] sent its groups (expected to be ignored)");
    Ok(())
}

async fn bob(
    alice_ready_rx: oneshot::Receiver<()>,
    carol_go_tx: oneshot::Sender<()>,
    done_tx: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let _ = alice_ready_rx.await;

    let session = new_session().await?;
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            NAMESPACE.to_string(),
            TRACK_NAME.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    tracing::info!(
        "[bob] subscribe ok, track_alias={}",
        subscription.track_alias()
    );

    let data_receiver = subscriber.accept_data_receiver(&subscription).await?;
    let mut factory: StreamDataReceiverFactory<QUIC> = match data_receiver {
        DataReceiver::Stream(f) => f,
        DataReceiver::Datagram(_) => anyhow::bail!("[bob] unexpected datagram"),
    };

    let mut alice_count = 0u64;
    let mut carol_count = 0u64;
    let mut max_alice_group = 0u64;
    let mut carol_go_tx = Some(carol_go_tx);

    // Collect for a bounded window so Carol's (ignored) objects have time to leak
    // through if the single-writer guard ever regressed.
    let collect = async {
        loop {
            let mut stream = match factory.next().await {
                Ok(s) => s,
                Err(_) => break,
            };
            loop {
                match stream.receive().await {
                    Ok(Subgroup::Header(h)) => tracing::info!("[bob] live group {}", h.group_id),
                    Ok(Subgroup::Object(field)) => {
                        if let SubgroupObject::Payload { data, .. } = field.subgroup_object {
                            let payload = String::from_utf8_lossy(&data).to_string();
                            if payload.starts_with("carol:") {
                                carol_count += 1;
                            } else {
                                alice_count += 1;
                                if let Some(g) = payload
                                    .strip_prefix("alice:g")
                                    .and_then(|s| s.split(':').next())
                                    .and_then(|s| s.parse::<u64>().ok())
                                {
                                    max_alice_group = max_alice_group.max(g);
                                }
                            }
                            tracing::info!("[bob] recv {}", payload);
                            if let Some(tx) = carol_go_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(6), collect).await;

    tracing::info!(
        "[bob] received alice={} carol={} max_alice_group={}",
        alice_count,
        carol_count,
        max_alice_group
    );
    assert_eq!(
        carol_count, 0,
        "carol's objects must not reach subscribers (first-writer-wins)"
    );
    assert!(
        max_alice_group >= LATE_GROUP,
        "alice must keep flowing after carol leaves: max group {} < {}",
        max_alice_group,
        LATE_GROUP
    );
    tracing::info!("[bob] OK: first-writer-wins holds and alice keeps flowing");
    let _ = done_tx.send(());
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    let (alice_ready_tx, alice_ready_rx) = oneshot::channel::<()>();
    let (carol_go_tx, carol_go_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();

    let alice_handle = tokio::spawn(alice(alice_ready_tx, done_rx));
    let mut bob_handle = tokio::spawn(bob(alice_ready_rx, carol_go_tx, done_tx));
    tokio::spawn(carol(carol_go_rx));

    // Success requires Bob's assertions to actually run, so always wait for Bob to
    // finish. Alice is a background producer; only surface her early exit if it is
    // an error.
    tokio::select! {
        r = &mut bob_handle => { r??; }
        r = alice_handle => {
            r??;
            bob_handle.await??;
        }
    }

    Ok(())
}
