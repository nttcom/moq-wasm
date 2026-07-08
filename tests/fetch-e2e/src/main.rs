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
//! When MOQT_E2E_RELAY_A_URL and MOQT_E2E_RELAY_B_URL are both set, two
//! additional cross-relay scenarios are run:
//!
//!   - Dave publishes on relay-A; Eve FETCHes from relay-B (no prior SUBSCRIBE).
//!     relay-B must forward the FETCH upstream to relay-A.
//!
//!   - Frank publishes on relay-A; Grace subscribes via relay-B then issues a
//!     Joining Fetch from relay-B.
//!
//! Run a relay on localhost:4433, then `cargo run -p fetch-e2e` from the repo
//! root. Any range that resolves to the wrong object set fails an assertion.

use std::env;
use std::net::ToSocketAddrs;
use std::str::FromStr;

use moqt::{
    ClientConfig, ContentExists, DataReceiver, Endpoint, ExtensionHeaders, Fetch,
    FetchDataReceiver, FetchObject, FetchOption, FilterType, GroupOrder, Location, QUIC, Session,
    StreamDataReceiverFactory, StreamDataSenderFactory, Subgroup, SubgroupId, SubgroupObject,
    SubscribeOption,
};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const NAMESPACE: &str = "room/alice";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const GROUPS: u64 = 3;
const OBJECTS_PER_GROUP: u64 = 5;

async fn new_session() -> anyhow::Result<Session<QUIC>> {
    let relay_url =
        env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    connect_to_relay(&relay_url).await
}

async fn connect_to_relay(relay_url: &str) -> anyhow::Result<Session<QUIC>> {
    let url = url::Url::from_str(relay_url).unwrap();
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
    start: Location,
    end: Location,
) -> anyhow::Result<Vec<(u64, u64)>> {
    run_fetch_with_label(subscriber, start, end, "bob").await
}

async fn run_fetch_with_label(
    subscriber: &mut moqt::Subscriber<QUIC>,
    start: Location,
    end: Location,
    label: &str,
) -> anyhow::Result<Vec<(u64, u64)>> {
    run_fetch_ns(subscriber, NAMESPACE, TRACK_NAME, start, end, label).await
}

async fn run_fetch_ns(
    subscriber: &mut moqt::Subscriber<QUIC>,
    namespace: &str,
    track_name: &str,
    start: Location,
    end: Location,
    label: &str,
) -> anyhow::Result<Vec<(u64, u64)>> {
    tracing::info!(
        "[{}] fetching {}/{} g{}:o{}..g{}:o{}",
        label,
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
                tracing::info!(
                    "[{}] g{}:o{} = {}",
                    label,
                    obj.group_id,
                    obj.object_id,
                    payload
                );
                received.push((obj.group_id, obj.object_id));
            }
            Ok(Fetch::End) => {
                tracing::info!(
                    "[{}] fetch done g{}:o{}..g{}:o{}",
                    label,
                    start.group_id,
                    start.object_id,
                    end.group_id,
                    end.object_id
                );
                break;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "[{}] fetch g{}:o{}..g{}:o{} failed: {}",
                    label,
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
    run_joining_fetch_with_label(subscriber, joining_request_id, joining_start, "carol").await
}

async fn run_joining_fetch_with_label(
    subscriber: &mut moqt::Subscriber<QUIC>,
    joining_request_id: u64,
    joining_start: u64,
    label: &str,
) -> anyhow::Result<Vec<(u64, u64)>> {
    tracing::info!(
        "[{}] joining fetch request_id={} joining_start={}",
        label,
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
                    "[{}] joining g{}:o{} = {}",
                    label,
                    obj.group_id,
                    obj.object_id,
                    payload
                );
                received.push((obj.group_id, obj.object_id));
            }
            Ok(Fetch::End) => {
                tracing::info!("[{}] joining fetch done", label);
                break;
            }
            Err(e) => {
                return Err(anyhow::anyhow!("[{}] joining fetch failed: {}", label, e));
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
            moqt::SessionEvent::Fetch(handler) => {
                tracing::info!("[alice] received Fetch request_id={}", handler.request_id);
                // Alice relies on the relay cache to serve FETCHes; no explicit handling needed.
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

/// Run a publisher event loop: accept SUBSCRIBE events and publish all groups for each.
/// Exits on Disconnected or after serving `max_subscribes` subscriptions.
/// Returns the session so the caller can keep it alive (keeping the namespace registered).
async fn publisher_event_loop(
    session: Session<QUIC>,
    label: &'static str,
    max_subscribes: usize,
) -> anyhow::Result<Session<QUIC>> {
    let mut served = 0;
    loop {
        match session.receive_event().await {
            Ok(moqt::SessionEvent::Subscribe(handler)) => {
                tracing::info!("[{}] received Subscribe", label);
                let track_alias = handler.ok(0, ContentExists::False).await?;
                let publication = handler.into_subscription(track_alias);
                let factory = session.publisher().create_stream(&publication);
                for group_id in 0..GROUPS {
                    let sender = factory.next().await?;
                    let header = sender.create_header(
                        group_id,
                        SubgroupId::None,
                        PUBLISHER_PRIORITY,
                        false,
                        false,
                    );
                    let mut stream = sender.send_header(header).await?;
                    for obj_id in 0..OBJECTS_PER_GROUP {
                        let payload = format!("{}-g{}:o{}", label, group_id, obj_id);
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
                    stream.close().await?;
                    tracing::info!("[{}] published group {}", label, group_id);
                }
                served += 1;
                if served >= max_subscribes {
                    break;
                }
            }
            Ok(moqt::SessionEvent::Disconnected()) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[{}] session error: {}", label, e);
                break;
            }
        }
    }
    Ok(session)
}

/// Drain a stream subscription until a given group_id arrives, then stop.
async fn drain_until_group(
    subscriber: &mut moqt::Subscriber<QUIC>,
    subscription: &moqt::Subscription,
    until_group: u64,
    label: &str,
) -> anyhow::Result<()> {
    let data_receiver = subscriber.accept_data_receiver(subscription).await?;
    let mut factory: StreamDataReceiverFactory<QUIC> = match data_receiver {
        DataReceiver::Stream(f) => f,
        DataReceiver::Datagram(_) => anyhow::bail!("[{}] unexpected datagram", label),
    };
    'drain: loop {
        let mut stream = factory.next().await?;
        loop {
            match stream.receive().await {
                Ok(Subgroup::Header(h)) => {
                    tracing::info!("[{}] live group_id={}", label, h.group_id);
                    if h.group_id >= until_group {
                        break 'drain;
                    }
                }
                Ok(Subgroup::Object(_)) => {}
                Err(_) => break,
            }
        }
    }
    Ok(())
}

/// Cross-relay scenario D: Dave publishes on relay-A; a local subscriber on relay-A
/// populates relay-A's cache; then Eve FETCHes from relay-B (no prior subscribe via
/// relay-B). relay-B has no local cache, so it must forward the FETCH upstream to relay-A.
async fn run_cross_relay_standalone_fetch(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[D] cross-relay standalone FETCH: Dave on relay-A, Eve on relay-B");

    let namespace = "room/dave-cross-fetch";
    let track_name = "data";

    // Dave connects to relay-A and registers the namespace.
    let dave_session = connect_to_relay(relay_a_url).await?;
    dave_session
        .publisher()
        .publish_namespace(namespace.to_string())
        .await?;
    tracing::info!("[dave] publish_namespace ok on relay-A");

    // Spawn Dave's event loop concurrently so it can respond to SUBSCRIBE while
    // the local subscriber is connecting.
    let dave_handle = tokio::spawn(publisher_event_loop(dave_session, "dave", 1));

    // A local subscriber on relay-A triggers the SUBSCRIBE→publish chain, populating
    // relay-A's cache. This subscriber is intentionally on relay-A, not relay-B, so
    // relay-B's cache remains empty for this track.
    let local_sub_session = connect_to_relay(relay_a_url).await?;
    let mut local_sub = local_sub_session.subscriber();
    // Keep subscription alive until Dave finishes so the relay retains the cache.
    let _local_subscription = local_sub
        .subscribe(
            namespace.to_string(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    tracing::info!("[local-sub] subscribe ok on relay-A");

    // Wait for Dave to finish publishing all groups. The session is returned so we can
    // keep it alive (keeping the namespace registered in Redis) while Eve FETCHes.
    let dave_session = dave_handle.await??;
    tracing::info!("[local-sub] Dave finished publishing; relay-A cache populated");
    // Small grace period to ensure the relay fully processes and stores the last group.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Eve connects to relay-B and FETCHes directly — no prior SUBSCRIBE via relay-B,
    // so relay-B has no cache for this track and must forward the FETCH upstream to relay-A.
    // Dave's session must remain alive so the namespace stays registered in Redis.
    let eve_session = connect_to_relay(relay_b_url).await?;
    let mut eve_sub = eve_session.subscriber();

    let received = run_fetch_ns(
        &mut eve_sub,
        namespace,
        track_name,
        Location {
            group_id: 0,
            object_id: 0,
        },
        Location {
            group_id: GROUPS - 1,
            object_id: OBJECTS_PER_GROUP,
        },
        "eve",
    )
    .await?;
    // Dave's session kept alive until here; dropping it now unregisters the namespace.
    drop(dave_session);

    let expected: Vec<(u64, u64)> = (0..GROUPS)
        .flat_map(|g| (0..OBJECTS_PER_GROUP).map(move |o| (g, o)))
        .collect();
    assert_eq!(
        received, expected,
        "[D] cross-relay FETCH must deliver all groups via upstream forwarding"
    );

    tracing::info!("[D] OK: cross-relay standalone FETCH passed");
    Ok(())
}

/// Cross-relay scenario E: Frank publishes on relay-A; Grace subscribes via relay-B
/// (cross-relay SUBSCRIBE populates relay-B's cache) then issues a Joining Fetch from relay-B.
async fn run_cross_relay_joining_fetch(relay_a_url: &str, relay_b_url: &str) -> anyhow::Result<()> {
    tracing::info!("[E] cross-relay joining FETCH: Frank on relay-A, Grace on relay-B");

    let namespace = "room/frank-cross-join";
    let track_name = "data";

    // Frank connects to relay-A and registers the namespace.
    let frank_session = connect_to_relay(relay_a_url).await?;
    frank_session
        .publisher()
        .publish_namespace(namespace.to_string())
        .await?;
    tracing::info!("[frank] publish_namespace ok on relay-A");

    // Spawn Frank's event loop concurrently.
    let frank_handle = tokio::spawn(publisher_event_loop(frank_session, "frank", 1));

    // Grace connects to relay-B and subscribes (triggers cross-relay SUBSCRIBE → relay-A).
    let grace_session = connect_to_relay(relay_b_url).await?;
    let mut grace_sub = grace_session.subscriber();

    let subscription = grace_sub
        .subscribe(
            namespace.to_string(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    tracing::info!(
        "[grace] subscribe ok on relay-B, request_id={}",
        subscription.request_id()
    );

    // Drain Grace's live stream until all groups arrive (relay-B cache populated).
    drain_until_group(&mut grace_sub, &subscription, GROUPS - 1, "grace").await?;
    tracing::info!("[grace] all groups received, issuing joining fetch");

    // Wait for Frank's publish loop to finish. Drop the returned session immediately.
    drop(frank_handle.await??);

    // Joining Fetch: 1 group back from g2, so groups 1 and 2 in full.
    let received =
        run_joining_fetch_with_label(&mut grace_sub, subscription.request_id(), 1, "grace").await?;
    let expected: Vec<(u64, u64)> = (0..OBJECTS_PER_GROUP)
        .map(|o| (1, o))
        .chain((0..OBJECTS_PER_GROUP).map(|o| (2, o)))
        .collect();
    assert_eq!(
        received, expected,
        "[E] cross-relay joining fetch must deliver groups 1 and 2 in full"
    );

    tracing::info!("[E] OK: cross-relay joining fetch passed");
    Ok(())
}

/// Cross-relay scenario F: relay-B cache miss for a track that does not exist
/// on relay-A either.  FETCH must be forwarded upstream and relay-A's FETCH_ERROR
/// must be propagated back to the downstream subscriber as an error.
async fn run_upstream_fetch_forwarding_data_not_found(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[F] cross-relay FETCH_ERROR: namespace on relay-A but track not published");

    // Publisher announces the namespace on relay-A so relay-B can resolve the
    // upstream route via Redis. No track is actually published.
    let pub_session = connect_to_relay(relay_a_url).await?;
    pub_session
        .publisher()
        .publish_namespace("room/fetch-not-found".to_string())
        .await?;

    // Give the namespace route time to propagate through Redis to relay-B.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let session_b = connect_to_relay(relay_b_url).await?;
    let mut sub_b = session_b.subscriber();

    let result = sub_b
        .fetch(
            "room/fetch-not-found".to_string(),
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
        "[F] expected FETCH_ERROR for track not found at upstream, but got Ok"
    );
    tracing::info!("[F] OK: FETCH_ERROR returned for nonexistent track (as expected)");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

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

    // Cross-relay scenarios run only when two relay URLs are provided.
    let relay_a_url = env::var("MOQT_E2E_RELAY_A_URL").ok();
    let relay_b_url = env::var("MOQT_E2E_RELAY_B_URL").ok();

    if let (Some(relay_a_url), Some(relay_b_url)) = (relay_a_url, relay_b_url) {
        if relay_a_url != relay_b_url {
            run_cross_relay_standalone_fetch(&relay_a_url, &relay_b_url).await?;
            run_cross_relay_joining_fetch(&relay_a_url, &relay_b_url).await?;
            run_upstream_fetch_forwarding_data_not_found(&relay_a_url, &relay_b_url).await?;
        } else {
            tracing::info!("relay-A and relay-B are the same; skipping cross-relay scenarios");
        }
    } else {
        tracing::info!(
            "MOQT_E2E_RELAY_A_URL or MOQT_E2E_RELAY_B_URL not set; skipping cross-relay scenarios"
        );
    }

    Ok(())
}
