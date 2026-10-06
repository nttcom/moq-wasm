//! Manual e2e for two publishers of one track (draft-14 §8.2).
//!
//! Alice publishes a track and Bob subscribes to it (data flows). Carol then
//! publishes the *same* Full Track Name with the same objects, as a redundant
//! publisher would, and disconnects after sending them. The relay ingests both
//! and deduplicates, so Bob must receive every object exactly once, and
//! Carol's departure must not stop Alice's later groups.
//!
//! The `handover` scenarios then check that a subscription survives its
//! publishers coming and going: a killed publisher does not hold back a new
//! SUBSCRIBE, and a publisher announcing the namespace later takes over when
//! the first one leaves, on the same relay and across relays.
//!
//! Run relays on localhost:4433 and localhost:4434 sharing a route registry
//! (`run.sh` starts both), then `cargo run -p multiple-publishers-e2e` from the
//! repo root. It prints `multiple publishers e2e passed` on success.

mod handover;

use std::collections::HashMap;
use std::env;
use std::time::Duration;

use moqt::{
    ClientConfig, DataReceiver, Endpoint, ExtensionHeaders, FilterType, GroupOrder, PublishOption,
    QUIC, Session, StreamDataReceiverFactory, StreamDataSenderFactory, Subgroup, SubgroupId,
    SubgroupObject, SubscribeOption,
};
use tokio::sync::oneshot;

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const DEFAULT_RELAY_B_URL: &str = "moqt://127.0.0.1:4434";
const NAMESPACE: &str = "anon/room/main";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const ALICE_GROUPS: u64 = 8;
const CAROL_GROUPS: u64 = 4;
const OBJECTS_PER_GROUP: u64 = 5;
// Alice sends this group only after Carol has joined and left, so receiving it
// proves Carol's departure did not stop Alice's ingest.
const LATE_GROUP: u64 = 4;

pub(crate) fn object_payload(group_id: u64, object_id: u64) -> String {
    format!("g{group_id}:o{object_id}")
}

fn relay_url() -> String {
    env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string())
}

async fn connect(relay_url: &str) -> anyhow::Result<Session> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: None,
    })?;
    endpoint.connect(relay_url).await?.await
}

async fn send_group(
    factory: &StreamDataSenderFactory,
    who: &str,
    group_id: u64,
) -> anyhow::Result<()> {
    let sender = factory.next().await?;
    let header = sender.create_header(group_id, SubgroupId::None, PUBLISHER_PRIORITY, false, false);
    let mut stream = sender.send_header(header).await?;
    for obj_id in 0..OBJECTS_PER_GROUP {
        let payload = object_payload(group_id, obj_id);
        let obj = stream.create_object_field(
            0,
            ExtensionHeaders::default(),
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
    let session = connect(&relay_url()).await?;
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

    let session = connect(&relay_url()).await?;
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
    tracing::info!("[carol] publish ok");
    let factory = publisher.create_stream(&subscription);
    for group_id in 0..CAROL_GROUPS {
        send_group(&factory, "carol", group_id).await?;
    }
    tracing::info!("[carol] sent its groups");
    Ok(())
}

async fn bob(
    alice_ready_rx: oneshot::Receiver<()>,
    carol_go_tx: oneshot::Sender<()>,
    done_tx: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let _ = alice_ready_rx.await;

    let session = connect(&relay_url()).await?;
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
    let mut factory: StreamDataReceiverFactory = match data_receiver {
        DataReceiver::Stream(f) => f,
        DataReceiver::Datagram(_) => anyhow::bail!("[bob] unexpected datagram"),
    };

    let mut received = HashMap::<String, u64>::new();
    let mut max_group = 0u64;
    let mut carol_go_tx = Some(carol_go_tx);

    let collect = async {
        loop {
            let mut stream = match factory.next().await {
                Ok(s) => s,
                Err(_) => break,
            };
            loop {
                match stream.receive().await {
                    Ok(Some(Subgroup::Header(h))) => {
                        tracing::info!("[bob] live group {}", h.group_id);
                        max_group = max_group.max(h.group_id);
                    }
                    Ok(Some(Subgroup::Object(field))) => {
                        if let SubgroupObject::Payload { data, .. } = field.subgroup_object {
                            let payload = String::from_utf8_lossy(&data).to_string();
                            tracing::info!("[bob] recv {}", payload);
                            *received.entry(payload).or_default() += 1;
                            if let Some(tx) = carol_go_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(6), collect).await;

    let duplicated: Vec<_> = received.iter().filter(|(_, count)| **count > 1).collect();
    tracing::info!(
        "[bob] received {} objects, max_group={}, duplicated={:?}",
        received.len(),
        max_group,
        duplicated
    );
    assert!(
        duplicated.is_empty(),
        "every object must reach the subscriber once: {duplicated:?}"
    );
    assert!(
        max_group >= LATE_GROUP,
        "alice must keep flowing after carol leaves: max group {max_group} < {LATE_GROUP}"
    );
    tracing::info!("[bob] OK: objects deduplicated and alice keeps flowing");
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
    let relay_url = relay_url();
    let relay_b_url =
        env::var("MOQT_E2E_RELAY_B_URL").unwrap_or_else(|_| DEFAULT_RELAY_B_URL.to_string());

    let args: Vec<String> = env::args().collect();
    if let [_, flag, track_namespace] = args.as_slice()
        && flag == handover::SERVE_NAMESPACE_ARG
    {
        return handover::serve_namespace_forever(&relay_url, track_namespace).await;
    }

    run_redundant_publishers_scenario().await?;
    handover::run_killed_publisher_scenario(&relay_url).await?;
    handover::run_handover_scenario(&relay_url, &relay_url, "handover").await?;
    handover::run_handover_scenario(&relay_url, &relay_b_url, "cross-relay-handover").await?;
    println!("multiple publishers e2e passed");
    Ok(())
}

async fn run_redundant_publishers_scenario() -> anyhow::Result<()> {
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
