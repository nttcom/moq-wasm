//! E2E for the relay's cache eviction (TTL object drain + post-drain track reclaim).
//!
//! Two independent tracks exercise the two eviction mechanisms. FETCH reads the
//! relay's object cache directly, so "present vs gone" is observed by fetching.
//!
//!   Phase A (object TTL + closed-group reclaim):
//!     A publisher sends one closed group (o0..o4) on track A. A live subscriber
//!     (Carol) stays connected to keep the track alive (strong_count >= 2).
//!       - fetch A/g0 right away          -> 5 objects (cached)
//!       - fetch A/g0 before TTL          -> 5 objects (not removed early)
//!       - fetch A/g0 after TTL+interval  -> 0 objects, but the fetch still
//!         succeeds because the track (held by Carol) is alive; only the
//!         closed, drained group was reclaimed.
//!
//!   Phase B (track reclaim after TTL drain, no holders):
//!     A publisher sends one closed group on track B. A subscriber (Bob) triggers
//!     the publish and holds the track during the baseline fetch, then leaves.
//!       - fetch B/g0 while Bob holds it  -> 5 objects
//!       - after Bob leaves, wait > interval but < TTL
//!       - fetch B/g0                     -> 5 objects: a track with fresh
//!         objects must keep serving FETCH even with no session holding it
//!         (reclaiming it here loses data, see the object-dedup E2E)
//!       - after TTL+interval             -> FETCH error (TrackNotFound): the
//!         objects drained via TTL, the emptied track was reclaimed.
//!
//! Run a relay on localhost:4433 with a short TTL, then `cargo run -p
//! cache-eviction-e2e` from the repo root:
//!
//!   RELAY_CACHE_TTL_SECS=5 RELAY_CACHE_EVICT_INTERVAL_SECS=1 cargo run -p relay
//!   cargo run -p cache-eviction-e2e
//!
//! The client sleeps below assume TTL=5s / interval=1s.

use std::{env, net::ToSocketAddrs, str::FromStr, time::Duration};

use moqt::{
    ClientConfig, ContentExists, Endpoint, ExtensionHeaders, Fetch, FetchDataReceiver, FetchOption,
    FilterType, GroupOrder, Location, QUIC, Session, StreamDataSenderFactory, SubgroupId,
    SubgroupObject, SubscribeOption,
};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const NAMESPACE_A: &str = "evict/a";
const NAMESPACE_B: &str = "evict/b";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const OBJECTS_PER_GROUP: u64 = 5;

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

/// Publishes one closed group (o0..o4) whenever a subscribe arrives, then keeps
/// the session alive as the upstream publisher.
async fn publisher(namespace: &'static str) -> anyhow::Result<()> {
    let session = new_session().await?;
    session
        .publisher()
        .publish_namespace(namespace.to_string())
        .await?;
    tracing::info!("[pub {}] publish_namespace ok", namespace);

    loop {
        match session.receive_event().await? {
            moqt::SessionEvent::Subscribe(handler) => {
                let track_alias = handler.ok(0, ContentExists::False).await?;
                let publication = handler.into_subscription(track_alias);
                let factory = session.publisher().create_stream(&publication);
                send_group(&factory, 0).await?;
                tracing::info!("[pub {}] group 0 published and closed", namespace);
            }
            moqt::SessionEvent::Disconnected() => break,
            _ => {}
        }
    }
    Ok(())
}

async fn send_group(factory: &StreamDataSenderFactory<QUIC>, group_id: u64) -> anyhow::Result<()> {
    let sender = factory.next().await?;
    let header = sender.create_header(group_id, SubgroupId::None, PUBLISHER_PRIORITY, false, false);
    let mut stream = sender.send_header(header).await?;
    for obj_id in 0..OBJECTS_PER_GROUP {
        let payload = format!("g{}:o{}", group_id, obj_id);
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
    }
    // Closing the stream FINs the subgroup, so the relay marks the group closed.
    stream.close().await
}

/// Subscribes live and returns the owning session so the caller controls when
/// the subscription (and thus the relay-side `Arc<TrackCache>`) is released.
async fn subscribe_live(namespace: &str) -> anyhow::Result<Session<QUIC>> {
    let session = new_session().await?;
    let mut subscriber = session.subscriber();
    subscriber
        .subscribe(
            namespace.to_string(),
            TRACK_NAME.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    tracing::info!("[sub {}] subscribe ok", namespace);
    Ok(session)
}

/// Fetches the whole of `group_id` on a fresh, short-lived session and returns
/// the received (group_id, object_id) pairs. A relay FETCH error (e.g. the
/// track was reclaimed) surfaces as `Err`.
async fn fetch_group(namespace: &str, group_id: u64) -> anyhow::Result<Vec<(u64, u64)>> {
    let session = new_session().await?;
    let mut subscriber = session.subscriber();
    // start = {g, 0}, end = {g, 0}: end object_id 0 means the entire group.
    let handle = subscriber
        .fetch(
            namespace.to_string(),
            TRACK_NAME.to_string(),
            Location {
                group_id,
                object_id: 0,
            },
            Location {
                group_id,
                object_id: 0,
            },
            FetchOption::default(),
        )
        .await?;
    let mut receiver: FetchDataReceiver<QUIC> = subscriber.accept_fetch_receiver(&handle).await?;
    let mut received = Vec::new();
    loop {
        match receiver.receive().await {
            Ok(Fetch::Header(_)) => {}
            Ok(Fetch::Object(obj)) => received.push((obj.group_id, obj.object_id)),
            Ok(Fetch::End) => break,
            Err(e) => return Err(anyhow::anyhow!("fetch receive failed: {}", e)),
        }
    }
    Ok(received)
}

fn full_group(group_id: u64) -> Vec<(u64, u64)> {
    (0..OBJECTS_PER_GROUP).map(|o| (group_id, o)).collect()
}

async fn run_scenario() -> anyhow::Result<()> {
    // Publishers register their namespaces and publish on demand.
    tokio::spawn(publisher(NAMESPACE_A));
    tokio::spawn(publisher(NAMESPACE_B));
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- Phase A: object TTL + closed-group reclaim (track kept alive) ----
    let carol = subscribe_live(NAMESPACE_A).await?;
    tokio::time::sleep(Duration::from_millis(800)).await; // let group 0 publish

    let baseline = fetch_group(NAMESPACE_A, 0).await?;
    assert_eq!(
        baseline,
        full_group(0),
        "[A] group 0 must be cached initially"
    );
    tracing::info!("[A] baseline: 5 objects cached");

    tokio::time::sleep(Duration::from_secs(2)).await; // still < TTL
    let before_ttl = fetch_group(NAMESPACE_A, 0).await?;
    assert_eq!(
        before_ttl,
        full_group(0),
        "[A] group 0 must still be present before TTL"
    );
    tracing::info!("[A] before TTL: still 5 objects (not removed early)");

    tokio::time::sleep(Duration::from_secs(4)).await; // now past TTL + interval
    let after_ttl = fetch_group(NAMESPACE_A, 0).await?;
    assert!(
        after_ttl.is_empty(),
        "[A] group 0 objects must be evicted by TTL, got {:?}",
        after_ttl
    );
    tracing::info!("[A] after TTL: 0 objects, but fetch still resolved (track alive) — OK");

    drop(carol); // release the track for good measure

    // ---- Phase B: track reclaim after TTL drain (no holders) ----
    let bob = subscribe_live(NAMESPACE_B).await?;
    tokio::time::sleep(Duration::from_millis(800)).await; // let group 0 publish

    let baseline_b = fetch_group(NAMESPACE_B, 0).await?;
    assert_eq!(
        baseline_b,
        full_group(0),
        "[B] group 0 must be cached while Bob holds the track"
    );
    tracing::info!("[B] baseline: 5 objects cached");

    drop(bob); // now nobody holds the track (strong_count -> 1)
    tokio::time::sleep(Duration::from_secs(3)).await; // > interval, still < TTL

    let unheld = fetch_group(NAMESPACE_B, 0).await?;
    assert_eq!(
        unheld,
        full_group(0),
        "[B] a fresh track must keep serving FETCH after all holders left"
    );
    tracing::info!("[B] after Bob left, before TTL: still 5 objects — OK");

    tokio::time::sleep(Duration::from_secs(4)).await; // objects now past TTL + interval
    match fetch_group(NAMESPACE_B, 0).await {
        Err(e) => {
            tracing::info!("[B] after TTL: fetch errored ({}) — track reclaimed, OK", e);
        }
        Ok(objects) => {
            anyhow::bail!(
                "[B] track should have drained via TTL and been reclaimed, but fetch returned {:?}",
                objects
            );
        }
    }

    tracing::info!("ALL OK: object TTL drain and post-drain track reclaim both verified");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    run_scenario().await
}
