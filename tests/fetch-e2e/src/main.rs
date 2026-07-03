//! E2E for the relay's FETCH range resolution against its object cache.
//!
//! Alice publishes 3 groups of 5 objects each (g0..g2, o0..o4) on a single
//! track. Bob subscribes live until group 2 arrives (so the relay cache is fully
//! populated), then issues three standalone FETCHes and asserts he receives
//! exactly the objects the requested ranges resolve to, in order:
//!
//!   - Fetch A [g0/o0, g1/o3): g0/o0..o4 + g1/o0..o2  (8 objects)
//!   - Fetch B [g1/o2, g2/o4): g1/o2..o4 + g2/o0..o3  (7 objects)
//!   - Fetch C [g1/o0, g1/o0): whole group 1, o0..o4  (5 objects)
//!
//! Carol then issues a Joining Fetch (1 group back from the largest at
//! subscribe, g2) and asserts she receives groups 1 and 2 in full (10 objects).
//!
//! Run a relay on localhost:4433, then `cargo run -p fetch-e2e` from the repo
//! root. Any range that resolves to the wrong object set fails an assertion.
//!
//! ## Multi-relay FETCH forwarding scenarios
//!
//! When MOQT_E2E_RELAY_A_URL and MOQT_E2E_RELAY_B_URL are set (pointing to two
//! distinct relays backed by shared Redis routing), two additional scenarios run:
//!
//!   1. relay-b has no local cache for the track. FETCH issued to relay-b must
//!      be forwarded upstream to relay-a, which HAS the objects in cache.
//!      Expected: FETCH_OK, all objects delivered to the end subscriber.
//!
//!   2. relay-b has no local cache. FETCH issued to relay-b is forwarded to
//!      relay-a, which does NOT have the requested track in cache.
//!      Expected: FETCH_ERROR returned to the end subscriber.

use std::env;
use std::net::ToSocketAddrs;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use moqt::{
    ClientConfig, ContentExists, DataReceiver, Endpoint, ExtensionHeaders, Fetch, FetchDataReceiver,
    FetchObject, FetchOption, FilterType, GroupOrder, Location, QUIC, Session,
    StreamDataReceiverFactory, StreamDataSenderFactory, Subgroup, SubgroupId, SubgroupObject,
    SubscribeOption,
};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const DEFAULT_RELAY_A_URL: &str = "moqt://127.0.0.1:4433";
const DEFAULT_RELAY_B_URL: &str = "moqt://127.0.0.1:4434";
const NAMESPACE: &str = "room/alice";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const GROUPS: u64 = 3;
const OBJECTS_PER_GROUP: u64 = 5;

async fn new_session() -> anyhow::Result<Session<QUIC>> {
    let relay_url =
        env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    new_session_to(&relay_url).await
}

async fn new_session_to(url: &str) -> anyhow::Result<Session<QUIC>> {
    let parsed = url::Url::from_str(url)?;
    let host = parsed.host_str().unwrap_or("127.0.0.1");
    let port = parsed.port().unwrap_or(4433);
    let remote = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve relay address for {url}"))?;
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
    })?;
    let connecting = endpoint.connect(remote, host).await?;
    connecting.await
}

fn run_id() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
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
        tracing::info!("[alice] sent g{}:o{}", group_id, obj_id);
    }
    stream.close().await
}

/// Runs one standalone FETCH and returns the received (group_id, object_id) in
/// delivery order. A receive error before `Fetch::End` is propagated.
async fn run_fetch(
    subscriber: &mut moqt::Subscriber<QUIC>,
    namespace: &str,
    track_name: &str,
    start: Location,
    end: Location,
) -> anyhow::Result<Vec<(u64, u64)>> {
    tracing::info!(
        "fetching {}/{} g{}:o{}..g{}:o{}",
        namespace,
        track_name,
        start.group_id,
        start.object_id,
        end.group_id,
        end.object_id
    );
    let handle = subscriber
        .fetch(
            namespace.to_string(),
            track_name.to_string(),
            start,
            end,
            FetchOption::default(),
        )
        .await?;
    let mut receiver: FetchDataReceiver<QUIC> = subscriber.accept_fetch_receiver(&handle).await?;
    let mut received = Vec::new();
    loop {
        match receiver.receive().await {
            Ok(Fetch::Header(_)) => {}
            Ok(Fetch::Object(obj)) => {
                let payload = match &obj.fetch_object {
                    FetchObject::Payload(b) => String::from_utf8_lossy(b).to_string(),
                    FetchObject::Status(s) => format!("{:?}", s),
                };
                tracing::info!("g{}:o{} = {}", obj.group_id, obj.object_id, payload);
                received.push((obj.group_id, obj.object_id));
            }
            Ok(Fetch::End) => {
                tracing::info!(
                    "fetch done g{}:o{}..g{}:o{}",
                    start.group_id,
                    start.object_id,
                    end.group_id,
                    end.object_id
                );
                break;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "fetch {}/{} g{}:o{}..g{}:o{} failed: {}",
                    namespace,
                    track_name,
                    start.group_id,
                    start.object_id,
                    end.group_id,
                    end.object_id,
                    e
                ));
            }
        }
    }
    Ok(received)
}

/// Runs the Joining Fetch and returns the received (group_id, object_id) in
/// delivery order. A receive error before `Fetch::End` is propagated.
async fn run_joining_fetch(
    subscriber: &mut moqt::Subscriber<QUIC>,
    joining_request_id: u64,
    joining_start: u64,
) -> anyhow::Result<Vec<(u64, u64)>> {
    tracing::info!(
        "[carol] joining fetch request_id={} joining_start={}",
        joining_request_id,
        joining_start
    );
    let handle = subscriber
        .fetch_relative_joining(joining_request_id, joining_start, FetchOption::default())
        .await?;
    let mut receiver: FetchDataReceiver<QUIC> = subscriber.accept_fetch_receiver(&handle).await?;
    let mut received = Vec::new();
    loop {
        match receiver.receive().await {
            Ok(Fetch::Header(_)) => {}
            Ok(Fetch::Object(obj)) => {
                let payload = match &obj.fetch_object {
                    FetchObject::Payload(b) => String::from_utf8_lossy(b).to_string(),
                    FetchObject::Status(s) => format!("{:?}", s),
                };
                tracing::info!(
                    "[carol] joining g{}:o{} = {}",
                    obj.group_id,
                    obj.object_id,
                    payload
                );
                received.push((obj.group_id, obj.object_id));
            }
            Ok(Fetch::End) => {
                tracing::info!("[carol] joining fetch done");
                break;
            }
            Err(e) => {
                return Err(anyhow::anyhow!("[carol] joining fetch failed: {}", e));
            }
        }
    }
    Ok(received)
}

async fn carol(
    ready_rx: tokio::sync::oneshot::Receiver<()>,
    done_tx: tokio::sync::oneshot::Sender<()>,
) -> anyhow::Result<()> {
    // Wait until Bob has finished his standalone fetches (cache is fully populated).
    let _ = ready_rx.await;

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
        "[carol] subscribe ok, request_id={}, content_exists={:?}",
        subscription.request_id(),
        subscription.content_exists()
    );

    // Joining Fetch: 1 group back from largest_at_subscribe (g2), so groups 1
    // and 2 in full.
    let received = run_joining_fetch(&mut subscriber, subscription.request_id(), 1).await?;
    let expected: Vec<(u64, u64)> = (0..OBJECTS_PER_GROUP)
        .map(|o| (1, o))
        .chain((0..OBJECTS_PER_GROUP).map(|o| (2, o)))
        .collect();
    assert_eq!(
        received, expected,
        "joining fetch (1 group back from g2) must deliver groups 1 and 2 in full"
    );

    tracing::info!("[carol] OK: joining fetch delivered groups 1 and 2 in full");
    let _ = done_tx.send(());
    Ok(())
}

async fn alice() -> anyhow::Result<()> {
    let session = new_session().await?;
    session
        .publisher()
        .publish_namespace(NAMESPACE.to_string())
        .await?;
    tracing::info!("[alice] publish_namespace ok");

    loop {
        match session.receive_event().await? {
            moqt::SessionEvent::Subscribe(handler) => {
                tracing::info!(
                    "[alice] received Subscribe for {}/{}",
                    handler.track_namespace,
                    handler.track_name
                );
                let track_alias = handler.ok(0, ContentExists::False).await?;
                let publication = handler.into_subscription(track_alias);
                let factory = session.publisher().create_stream(&publication);
                for group_id in 0..GROUPS {
                    send_group(&factory, group_id).await?;
                }
                tracing::info!("[alice] all groups published");
            }
            moqt::SessionEvent::Disconnected() => break,
            _ => {}
        }
    }
    Ok(())
}

async fn bob(
    carol_ready_tx: tokio::sync::oneshot::Sender<()>,
    carol_done_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

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

    'detect: loop {
        let mut stream = factory.next().await?;
        loop {
            match stream.receive().await {
                Ok(Subgroup::Header(h)) => {
                    tracing::info!("[bob] live group_id={}", h.group_id);
                    if h.group_id >= 2 {
                        tracing::info!("[bob] group 2 detected, relay cache ready");
                        break 'detect;
                    }
                }
                Ok(Subgroup::Object(_)) => {}
                Err(_) => break, // stream ended, move to next group
            }
        }
    }

    tracing::info!("[bob] detect done, issuing fetches");

    // fetch A: [g0/o0, g1/o3) = g0/o0..g0/o4 + g1/o0..g1/o2 (8 objects)
    let received_a = run_fetch(
        &mut subscriber,
        NAMESPACE,
        TRACK_NAME,
        Location {
            group_id: 0,
            object_id: 0,
        },
        Location {
            group_id: 1,
            object_id: 3,
        },
    )
    .await?;
    let expected_a: Vec<(u64, u64)> = (0..OBJECTS_PER_GROUP)
        .map(|o| (0, o))
        .chain((0..3).map(|o| (1, o)))
        .collect();
    assert_eq!(
        received_a, expected_a,
        "fetch A [g0/o0, g1/o3) must resolve to g0 in full + g1/o0..o2"
    );
    // fetch B: [g1/o2, g2/o4) = g1/o2..g1/o4 + g2/o0..g2/o3 (7 objects)
    let received_b = run_fetch(
        &mut subscriber,
        NAMESPACE,
        TRACK_NAME,
        Location {
            group_id: 1,
            object_id: 2,
        },
        Location {
            group_id: 2,
            object_id: 4,
        },
    )
    .await?;
    let expected_b: Vec<(u64, u64)> = (2..OBJECTS_PER_GROUP)
        .map(|o| (1, o))
        .chain((0..4).map(|o| (2, o)))
        .collect();
    assert_eq!(
        received_b, expected_b,
        "fetch B [g1/o2, g2/o4) must resolve to g1/o2..o4 + g2/o0..o3"
    );
    // fetch C: [g1/o0, g1/o0) = entire group 1 (object_id==0 = whole group, 5 objects)
    let received_c = run_fetch(
        &mut subscriber,
        NAMESPACE,
        TRACK_NAME,
        Location {
            group_id: 1,
            object_id: 0,
        },
        Location {
            group_id: 1,
            object_id: 0,
        },
    )
    .await?;
    let expected_c: Vec<(u64, u64)> = (0..OBJECTS_PER_GROUP).map(|o| (1, o)).collect();
    assert_eq!(
        received_c, expected_c,
        "fetch C [g1/o0, g1/o0) must resolve to the whole of group 1"
    );
    tracing::info!("[bob] OK: all three standalone fetches resolved to the expected objects");

    // Signal Carol that standalone fetches are done and the cache is fully populated.
    let _ = carol_ready_tx.send(());
    // Keep this session alive until Carol completes her Joining Fetch. Carol only
    // sends on this channel once her assertions pass; a dropped sender means her
    // task failed (it panicked or returned an error), so fail the whole run.
    carol_done_rx
        .await
        .map_err(|_| anyhow::anyhow!("carol's joining fetch did not pass"))?;

    tracing::info!("[bob] all done");
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-relay FETCH forwarding helpers
// ---------------------------------------------------------------------------

/// Publisher that handles one SUBSCRIBE by sending `groups` groups, then exits.
/// Used in the multi-relay forwarding scenarios where the relay-a cache must be
/// populated before a downstream relay-b issues a FETCH.
async fn upstream_publisher(session: Session<QUIC>, groups: u64) -> anyhow::Result<()> {
    loop {
        match session.receive_event().await? {
            moqt::SessionEvent::Subscribe(handler) => {
                tracing::info!(
                    "[upstream-pub] SUBSCRIBE for {}/{}",
                    handler.track_namespace,
                    handler.track_name
                );
                let track_alias = handler.ok(0, ContentExists::False).await?;
                let publication = handler.into_subscription(track_alias);
                let factory = session.publisher().create_stream(&publication);
                for group_id in 0..groups {
                    send_group(&factory, group_id).await?;
                }
                tracing::info!("[upstream-pub] all {} groups sent", groups);
                return Ok(());
            }
            moqt::SessionEvent::Disconnected() => return Ok(()),
            _ => {}
        }
    }
}

/// Subscribes to `namespace/TRACK_NAME` on `relay_url` and drains exactly
/// `groups` subgroup streams to completion, ensuring the relay's cache is fully
/// populated before returning.
async fn drain_groups_from_relay(
    relay_url: &str,
    namespace: &str,
    groups: u64,
) -> anyhow::Result<()> {
    let session = new_session_to(relay_url).await?;
    let mut subscriber = session.subscriber();
    let subscription = subscriber
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

    let data_receiver = subscriber.accept_data_receiver(&subscription).await?;
    let mut factory: StreamDataReceiverFactory<QUIC> = match data_receiver {
        DataReceiver::Stream(f) => f,
        DataReceiver::Datagram(_) => anyhow::bail!("[drain] unexpected datagram"),
    };

    for _ in 0..groups {
        let mut stream = factory.next().await?;
        loop {
            match stream.receive().await {
                Ok(Subgroup::Header(h)) => {
                    tracing::info!("[drain] group {}", h.group_id);
                }
                Ok(Subgroup::Object(_)) => {}
                Err(_) => break, // stream closed — move to the next group
            }
        }
    }
    tracing::info!("[drain] {} groups drained; relay cache populated", groups);
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-relay scenario 1: relay-b cache miss → FETCH forwarded upstream →
// relay-a HAS the data → FETCH_OK, all objects delivered
// ---------------------------------------------------------------------------

/// relay-b has no local cache for the track.  A FETCH issued to relay-b must
/// be forwarded upstream to relay-a (which populated its cache via a drain
/// subscriber).  The end subscriber on relay-b must receive all published
/// objects.
///
/// This test FAILS with the current implementation because relay-b returns
/// FETCH_ERROR immediately on a cache miss without forwarding upstream.
async fn run_upstream_fetch_forwarding_data_exists(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    let ns = format!("fetch-fwd-exists/{}", run_id());
    tracing::info!(
        "[fetch-fwd-exists] namespace={} relay_a={} relay_b={}",
        ns,
        relay_a_url,
        relay_b_url
    );

    // Publisher connects to relay-a and waits for a SUBSCRIBE to send groups.
    let pub_session = new_session_to(relay_a_url).await?;
    pub_session
        .publisher()
        .publish_namespace(ns.clone())
        .await?;
    let pub_handle = tokio::spawn(upstream_publisher(pub_session, GROUPS));

    // Drain subscriber on relay-a: triggers publisher → relay-a cache fills.
    drain_groups_from_relay(relay_a_url, &ns, GROUPS).await?;
    tracing::info!("[fetch-fwd-exists] relay-a cache populated");

    // End subscriber on relay-b — relay-b has no local cache for this track.
    // With upstream FETCH forwarding implemented, relay-b must route the FETCH
    // to relay-a and serve the cached objects back to this subscriber.
    let session_b = new_session_to(relay_b_url).await?;
    let mut sub_b = session_b.subscriber();

    // Request the full range: all GROUPS groups, all OBJECTS_PER_GROUP objects.
    // End location object_id = OBJECTS_PER_GROUP (exclusive) covers o0..o(N-1).
    let received = run_fetch(
        &mut sub_b,
        &ns,
        TRACK_NAME,
        Location {
            group_id: 0,
            object_id: 0,
        },
        Location {
            group_id: GROUPS - 1,
            object_id: OBJECTS_PER_GROUP,
        },
    )
    .await?;

    let expected: Vec<(u64, u64)> = (0..GROUPS)
        .flat_map(|g| (0..OBJECTS_PER_GROUP).map(move |o| (g, o)))
        .collect();
    assert_eq!(
        received,
        expected,
        "[fetch-fwd-exists] upstream FETCH forwarding must deliver all {} objects",
        GROUPS * OBJECTS_PER_GROUP,
    );

    tracing::info!(
        "[fetch-fwd-exists] OK: {} objects received via upstream FETCH forwarding",
        received.len()
    );
    pub_handle.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-relay scenario 2: relay-b cache miss → FETCH forwarded upstream →
// relay-a does NOT have the track → FETCH_ERROR
// ---------------------------------------------------------------------------

/// relay-b has no local cache.  A FETCH issued to relay-b for a track that was
/// never published on relay-a must be forwarded upstream and relay-a must
/// respond with FETCH_ERROR (track not found in cache).  The end subscriber
/// must receive FETCH_ERROR (subscriber.fetch() returns Err).
///
/// With the current implementation relay-b also returns FETCH_ERROR, but
/// without contacting relay-a (local cache miss short-circuit).  This test
/// passes both before and after the implementation — it guards against
/// regressions where FETCH_ERROR is accidentally suppressed.
async fn run_upstream_fetch_forwarding_data_not_found(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    let ns = format!("fetch-fwd-notfound/{}", run_id());
    tracing::info!(
        "[fetch-fwd-notfound] namespace={} relay_a={} relay_b={}",
        ns,
        relay_a_url,
        relay_b_url
    );

    // Publisher announces the namespace on relay-a so relay-b can resolve the
    // upstream route via Redis.  No track is actually published.
    let pub_session = new_session_to(relay_a_url).await?;
    pub_session
        .publisher()
        .publish_namespace(ns.clone())
        .await?;

    // Give the namespace route time to propagate through Redis to relay-b.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // End subscriber on relay-b FETCHes a track that was never published.
    // relay-b must forward the FETCH upstream and receive FETCH_ERROR from
    // relay-a (or return FETCH_ERROR itself on cache miss — either way the
    // subscriber must observe an error).
    let session_b = new_session_to(relay_b_url).await?;
    let mut sub_b = session_b.subscriber();

    let result = sub_b
        .fetch(
            ns.clone(),
            "nonexistent-track".to_string(),
            Location {
                group_id: 0,
                object_id: 0,
            },
            Location {
                group_id: 0,
                object_id: OBJECTS_PER_GROUP,
            },
            FetchOption::default(),
        )
        .await;

    assert!(
        result.is_err(),
        "[fetch-fwd-notfound] expected FETCH_ERROR for track not found at upstream, but got Ok"
    );
    tracing::info!(
        "[fetch-fwd-notfound] OK: FETCH_ERROR returned for nonexistent track (as expected)"
    );
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    // -----------------------------------------------------------------------
    // Single-relay scenarios (existing)
    // -----------------------------------------------------------------------
    let (carol_ready_tx, carol_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let (carol_done_tx, carol_done_rx) = tokio::sync::oneshot::channel::<()>();

    let alice_handle = tokio::spawn(alice());
    let mut bob_handle = tokio::spawn(bob(carol_ready_tx, carol_done_rx));
    tokio::spawn(carol(carol_ready_rx, carol_done_tx));

    // Success requires Bob's (and, through him, Carol's) assertions to actually
    // run, so always wait for Bob to finish. Alice is a background producer; only
    // surface her early exit if it is an error.
    tokio::select! {
        r = &mut bob_handle => { r??; }
        r = alice_handle => {
            r??;
            bob_handle.await??;
        }
    }

    // -----------------------------------------------------------------------
    // Multi-relay FETCH forwarding scenarios
    // -----------------------------------------------------------------------
    let relay_a_url = env::var("MOQT_E2E_RELAY_A_URL")
        .or_else(|_| env::var("MOQT_E2E_RELAY_URL"))
        .unwrap_or_else(|_| DEFAULT_RELAY_A_URL.to_string());
    let relay_b_url = env::var("MOQT_E2E_RELAY_B_URL")
        .unwrap_or_else(|_| DEFAULT_RELAY_B_URL.to_string());

    tracing::info!(
        "running multi-relay FETCH forwarding scenarios (relay_a={} relay_b={})",
        relay_a_url,
        relay_b_url
    );

    // Scenario 1: data exists at upstream relay → FETCH_OK
    tokio::time::timeout(
        Duration::from_secs(30),
        run_upstream_fetch_forwarding_data_exists(&relay_a_url, &relay_b_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("upstream FETCH forwarding (data-exists) timed out"))??;

    // Scenario 2: track absent at upstream relay → FETCH_ERROR
    tokio::time::timeout(
        Duration::from_secs(30),
        run_upstream_fetch_forwarding_data_not_found(&relay_a_url, &relay_b_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("upstream FETCH forwarding (data-not-found) timed out"))??;

    tracing::info!("all fetch-e2e scenarios passed");
    Ok(())
}
