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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use moqt::{
    ClientConfig, ContentExists, DataReceiver, Endpoint, ExtensionHeaders, Fetch,
    FetchDataReceiver, FetchObject, FetchOption, FilterType, GroupOrder, Location, PublishOption,
    QUIC, Session, StreamDataReceiverFactory, StreamDataSenderFactory, Subgroup, SubgroupId,
    SubgroupObject, SubscribeOption,
};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const GROUPS: u64 = 3;
const OBJECTS_PER_GROUP: u64 = 5;

#[derive(Debug, Clone)]
struct FetchRunResult {
    objects: Vec<(u64, u64)>,
    end_location: Location,
}

fn unique_namespace(label: &str) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("room/{}-{}-{}", label, std::process::id(), now_ms)
}

async fn new_session() -> anyhow::Result<Session<QUIC>> {
    let relay_url =
        env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    connect_to_relay(&relay_url).await
}

async fn connect_to_relay(relay_url: &str) -> anyhow::Result<Session<QUIC>> {
    let url = url::Url::from_str(relay_url)
        .with_context(|| format!("failed to parse relay URL: {relay_url}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("relay URL has no host: {relay_url}"))?;
    let remote = (host, url.port().unwrap_or(4433))
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve relay address: {relay_url}"))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("relay URL resolved no socket address: {relay_url}"))?;
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
            ExtensionHeaders::default(),
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
    start: Location,
    end: Location,
) -> anyhow::Result<FetchRunResult> {
    run_fetch_with_label(subscriber, namespace, start, end, "bob").await
}

async fn run_fetch_with_label(
    subscriber: &mut moqt::Subscriber<QUIC>,
    namespace: &str,
    start: Location,
    end: Location,
    label: &str,
) -> anyhow::Result<FetchRunResult> {
    run_fetch_ns(subscriber, namespace, TRACK_NAME, start, end, label).await
}

async fn run_fetch_ns(
    subscriber: &mut moqt::Subscriber<QUIC>,
    namespace: &str,
    track_name: &str,
    start: Location,
    end: Location,
    label: &str,
) -> anyhow::Result<FetchRunResult> {
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
    let end_location = handle.end_location;
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
    Ok(FetchRunResult {
        objects: received,
        end_location,
    })
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
    namespace: String,
    ready_rx: tokio::sync::oneshot::Receiver<()>,
    done_tx: tokio::sync::oneshot::Sender<()>,
) -> anyhow::Result<()> {
    // Wait until Bob has finished his standalone fetches (cache is fully populated).
    let _ = ready_rx.await;

    let session = new_session().await?;
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            namespace.clone(),
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

async fn alice(namespace: String) -> anyhow::Result<()> {
    let session = new_session().await?;
    session
        .publisher()
        .publish_namespace(namespace.clone())
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
    namespace: String,
    carol_ready_tx: tokio::sync::oneshot::Sender<()>,
    carol_done_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    tokio::time::sleep(Duration::from_secs(1)).await;

    let session = new_session().await?;

    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            namespace.clone(),
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

    // Drain every group's stream to completion. QUIC gives no cross-stream
    // ordering, so fetching on first sight of g2 can race streams that have
    // not even reached the relay; a fetch for an un-arrived group is unknown
    // status and correctly goes upstream (spec §9.16), which this topology's
    // FETCH-less publisher cannot serve. The in-flight race itself is covered
    // by relay unit tests (resolve + delivery wait).
    let mut closed_groups = std::collections::HashSet::new();
    while closed_groups.len() < GROUPS as usize {
        let mut stream = factory.next().await?;
        let mut group_id = None;
        loop {
            match stream.receive().await {
                Ok(Some(Subgroup::Header(h))) => {
                    tracing::info!("[bob] live group_id={}", h.group_id);
                    group_id = Some(h.group_id);
                }
                Ok(Some(Subgroup::Object(_))) => {}
                Ok(None) | Err(_) => break, // stream ended: this group is fully received
            }
        }
        if let Some(group_id) = group_id {
            closed_groups.insert(group_id);
        }
    }

    tracing::info!("[bob] detect done, issuing fetches");

    // fetch A: [g0/o0, g1/o3) = g0/o0..g0/o4 + g1/o0..g1/o2 (8 objects)
    let received_a = run_fetch(
        &mut subscriber,
        &namespace,
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
        received_a.objects, expected_a,
        "fetch A [g0/o0, g1/o3) must resolve to g0 in full + g1/o0..o2"
    );
    // fetch B: [g1/o2, g2/o4) = g1/o2..g1/o4 + g2/o0..g2/o3 (7 objects)
    let received_b = run_fetch(
        &mut subscriber,
        &namespace,
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
        received_b.objects, expected_b,
        "fetch B [g1/o2, g2/o4) must resolve to g1/o2..o4 + g2/o0..o3"
    );
    // fetch C: [g1/o0, g1/o0) = entire group 1 (object_id==0 = whole group, 5 objects)
    let received_c = run_fetch(
        &mut subscriber,
        &namespace,
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
        received_c.objects, expected_c,
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
                            ExtensionHeaders::default(),
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

/// Drain a stream subscription until all objects in a given group arrive.
async fn drain_until_group_complete(
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
    loop {
        let mut stream = factory.next().await?;
        let mut current_group = None;
        let mut target_group_objects = 0;
        loop {
            match stream.receive().await {
                Ok(Some(Subgroup::Header(h))) => {
                    tracing::info!("[{}] live group_id={}", label, h.group_id);
                    current_group = Some(h.group_id);
                }
                Ok(Some(Subgroup::Object(_))) => {
                    if current_group == Some(until_group) {
                        target_group_objects += 1;
                    }
                }
                Ok(None) | Err(_) => {
                    if current_group == Some(until_group)
                        && target_group_objects >= OBJECTS_PER_GROUP
                    {
                        return Ok(());
                    }
                    break;
                }
            }
        }
    }
}

/// Cross-relay scenario D: Dave publishes on relay-A; a local subscriber on relay-A
/// populates relay-A's cache; then Eve FETCHes from relay-B (no prior subscribe via
/// relay-B). relay-B has no local cache, so it must forward the FETCH upstream to relay-A.
async fn run_cross_relay_standalone_fetch(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[D] cross-relay standalone FETCH: Dave on relay-A, Eve on relay-B");

    let namespace = unique_namespace("dave-cross-fetch");
    let track_name = "data";

    // Dave connects to relay-A and registers the namespace.
    let dave_session = connect_to_relay(relay_a_url).await?;
    dave_session
        .publisher()
        .publish_namespace(namespace.to_string())
        .await?;
    tracing::info!("[dave] publish_namespace ok on relay-A");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Spawn Dave's event loop concurrently so it can respond to SUBSCRIBE while
    // the local subscriber is connecting.
    let dave_handle = tokio::spawn(publisher_event_loop(dave_session, "dave", 1));

    // A local subscriber on relay-A triggers the SUBSCRIBE→publish chain, populating
    // relay-A's cache. This subscriber is intentionally on relay-A, not relay-B, so
    // relay-B's cache remains empty for this track.
    let local_sub_session = connect_to_relay(relay_a_url).await?;
    let mut local_sub = local_sub_session.subscriber();
    // Keep subscription alive until Dave finishes so the relay retains the cache.
    let local_subscription = local_sub
        .subscribe(
            namespace.clone(),
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

    // Drain through g2 before FETCH so relay-A's cache is deterministically populated.
    drain_until_group_complete(&mut local_sub, &local_subscription, GROUPS - 1, "local-sub")
        .await?;

    // Wait for Dave to finish publishing all groups. The session is returned so we can
    // keep it alive (keeping the namespace registered in Redis) while Eve FETCHes.
    let dave_session = dave_handle.await??;
    tracing::info!("[local-sub] Dave finished publishing; relay-A cache populated");

    // Eve connects to relay-B and FETCHes directly — no prior SUBSCRIBE via relay-B,
    // so relay-B has no cache for this track and must forward the FETCH upstream to relay-A.
    // Dave's session must remain alive so the namespace stays registered in Redis.
    let eve_session = connect_to_relay(relay_b_url).await?;
    let mut eve_sub = eve_session.subscriber();

    let received = run_fetch_ns(
        &mut eve_sub,
        &namespace,
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
        received.objects, expected,
        "[D] cross-relay FETCH must deliver all groups via upstream forwarding"
    );

    tracing::info!("[D] OK: cross-relay standalone FETCH passed");
    Ok(())
}

/// Cross-relay scenario E: Frank publishes on relay-A; Grace subscribes via relay-B
/// (cross-relay SUBSCRIBE populates relay-B's cache) then issues a Joining Fetch from relay-B.
async fn run_cross_relay_joining_fetch(relay_a_url: &str, relay_b_url: &str) -> anyhow::Result<()> {
    tracing::info!("[E] cross-relay joining FETCH: Frank on relay-A, Grace on relay-B");

    let namespace = unique_namespace("frank-cross-join");
    let track_name = "data";

    // Frank publishes through PUBLISH before Grace subscribes. A warm subscriber
    // on relay-B joins after g0, receives g1/g2, and populates relay-B's cache.
    let frank_session = connect_to_relay(relay_a_url).await?;
    let frank_publisher = frank_session.publisher();
    frank_publisher.publish_namespace(namespace.clone()).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let frank_publication = frank_publisher
        .publish(
            namespace.clone(),
            track_name.to_string(),
            PublishOption::default(),
        )
        .await?;
    let frank_factory = frank_publisher.create_stream(&frank_publication);
    send_group(&frank_factory, 0).await?;

    let warm_session = connect_to_relay(relay_b_url).await?;
    let mut warm_sub = warm_session.subscriber();
    let warm_subscription = warm_sub
        .subscribe(
            namespace.clone(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;

    send_group(&frank_factory, 1).await?;
    send_group(&frank_factory, 2).await?;
    drain_until_group_complete(&mut warm_sub, &warm_subscription, GROUPS - 1, "warm").await?;

    // Grace connects to relay-B and subscribes (triggers cross-relay SUBSCRIBE → relay-A).
    let grace_session = connect_to_relay(relay_b_url).await?;
    let mut grace_sub = grace_session.subscriber();

    let subscription = grace_sub
        .subscribe(
            namespace.clone(),
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

    tracing::info!("[grace] issuing joining fetch from warmed relay-B cache");

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
    drop(frank_session);

    tracing::info!("[E] OK: cross-relay joining fetch passed");
    Ok(())
}

/// Cross-relay scenario G: relay-B subscribes after relay-A already has earlier
/// groups, so relay-B is missing the head locally. A full-range standalone FETCH
/// must fill via upstream once, then be served from relay-B's cache the second time.
async fn run_cross_relay_leading_gap_fetch(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[G] cross-relay leading-gap FETCH: fill once, then serve locally");

    let namespace = unique_namespace("leading-gap-fetch");
    let track_name = "data";
    let publisher_session = connect_to_relay(relay_a_url).await?;
    publisher_session
        .publisher()
        .publish_namespace(namespace.clone())
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let publisher_handle = tokio::spawn(publisher_event_loop(publisher_session, "heidi", 1));

    let local_sub_session = connect_to_relay(relay_a_url).await?;
    let mut local_sub = local_sub_session.subscriber();
    let local_subscription = local_sub
        .subscribe(
            namespace.clone(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    drain_until_group_complete(&mut local_sub, &local_subscription, GROUPS - 1, "local-sub")
        .await?;
    let publisher_session = publisher_handle.await??;

    let relay_b_session = connect_to_relay(relay_b_url).await?;
    let mut relay_b_subscriber = relay_b_session.subscriber();
    let subscription = relay_b_subscriber
        .subscribe(
            namespace.clone(),
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
        "[gap-sub] subscribe ok on relay-B, request_id={}",
        subscription.request_id()
    );

    let fetch_session = connect_to_relay(relay_b_url).await?;
    let mut fetch_subscriber = fetch_session.subscriber();
    let requested_end = Location {
        group_id: GROUPS + 10,
        object_id: 0,
    };
    let expected: Vec<(u64, u64)> = (0..GROUPS)
        .flat_map(|g| (0..OBJECTS_PER_GROUP).map(move |o| (g, o)))
        .collect();
    let expected_end = Location {
        group_id: GROUPS - 1,
        object_id: OBJECTS_PER_GROUP,
    };

    let first = run_fetch_ns(
        &mut fetch_subscriber,
        &namespace,
        track_name,
        Location {
            group_id: 0,
            object_id: 0,
        },
        requested_end,
        "gap-first",
    )
    .await?;
    assert_eq!(
        first.objects, expected,
        "[G] first full-range fetch must fill the leading gap from upstream"
    );
    assert_eq!(
        first.end_location, expected_end,
        "[G] first FETCH_OK End Location must be clamped to upstream largest + 1"
    );

    let started = std::time::Instant::now();
    let second = run_fetch_ns(
        &mut fetch_subscriber,
        &namespace,
        track_name,
        Location {
            group_id: 0,
            object_id: 0,
        },
        requested_end,
        "gap-second",
    )
    .await?;
    tracing::info!("[G] second fetch elapsed: {:?}", started.elapsed());
    assert_eq!(
        second.objects, expected,
        "[G] second full-range fetch must be served from relay-B cache"
    );
    assert_eq!(
        second.end_location, expected_end,
        "[G] second FETCH_OK End Location must remain clamped"
    );

    drop(publisher_session);
    Ok(())
}

/// Cross-relay scenario F: relay-B cache miss for a track that does not exist
/// on relay-A either. The FETCH is forwarded upstream all the way to the
/// publisher client, whose unhandled-fetch auto-reject (NOT_SUPPORTED) must
/// come back promptly as FETCH_ERROR — and, critically, the publisher's
/// session must survive the failed FETCH (no timeout-driven session close).
async fn run_upstream_fetch_forwarding_data_not_found(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[F] cross-relay FETCH_ERROR: namespace on relay-A but track not published");
    let namespace = unique_namespace("fetch-not-found");

    // Publisher announces the namespace on relay-A. Its event loop drains
    // events, so a forwarded FETCH handler is dropped and auto-rejected, and
    // it serves one SUBSCRIBE afterwards for the liveness check.
    let pub_session = connect_to_relay(relay_a_url).await?;
    pub_session
        .publisher()
        .publish_namespace(namespace.clone())
        .await?;
    let publisher_handle = tokio::spawn(publisher_event_loop(pub_session, "nf-pub", 1));

    // Give the namespace route time to propagate through Redis to relay-B.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let session_b = connect_to_relay(relay_b_url).await?;
    let mut sub_b = session_b.subscriber();

    let started = std::time::Instant::now();
    let result = sub_b
        .fetch(
            namespace.clone(),
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
    let elapsed = started.elapsed();

    assert!(
        result.is_err(),
        "[F] expected FETCH_ERROR for track not found at upstream, but got Ok"
    );
    // The publisher client auto-rejects the unhandled FETCH, so the error must
    // arrive well before any control-message timeout would fire.
    assert!(
        elapsed < Duration::from_secs(5),
        "[F] FETCH_ERROR must come from the publisher's auto-reject, not a timeout (took {elapsed:?})"
    );
    tracing::info!("[F] FETCH_ERROR returned in {:?}", elapsed);

    // Liveness: the failed FETCH must not have torn down the publisher session
    // or the inter-relay session. A fresh cross-relay subscription must still
    // reach the publisher and deliver all groups.
    let live_session = connect_to_relay(relay_b_url).await?;
    let mut live_sub = live_session.subscriber();
    let live_subscription = live_sub
        .subscribe(
            namespace.clone(),
            TRACK_NAME.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    drain_until_group_complete(&mut live_sub, &live_subscription, GROUPS - 1, "nf-live").await?;
    drop(publisher_handle.await??);

    tracing::info!("[F] OK: prompt FETCH_ERROR and the publisher session survived");
    Ok(())
}

/// Cross-relay scenario H: relay-B's cache is completely cold (no prior FETCH
/// or subscriber warmed it) and the publisher has already finished, so the
/// upstream SUBSCRIBE triggered by the cold subscriber delivers no live
/// objects. Its Relative Joining Fetch must be forwarded upstream to relay-A
/// as a standalone FETCH with the pre-resolved absolute range (draft-14
/// §9.16.2.1) and fill relay-B's cache; before that fix relay-B answered with
/// FETCH_ERROR InternalError.
async fn run_cross_relay_cold_cache_joining_fetch(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[H] cross-relay cold-cache joining FETCH: fill from relay-A via upstream");

    let namespace = unique_namespace("cold-join-fetch");
    let track_name = "data";

    // Publisher on relay-A; a local subscriber on relay-A drains every group so
    // relay-A's cache is deterministically populated while relay-B stays cold.
    let publisher_session = connect_to_relay(relay_a_url).await?;
    publisher_session
        .publisher()
        .publish_namespace(namespace.clone())
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let publisher_handle = tokio::spawn(publisher_event_loop(publisher_session, "ivan", 1));

    let local_sub_session = connect_to_relay(relay_a_url).await?;
    let mut local_sub = local_sub_session.subscriber();
    let local_subscription = local_sub
        .subscribe(
            namespace.clone(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    drain_until_group_complete(&mut local_sub, &local_subscription, GROUPS - 1, "local-sub")
        .await?;
    let publisher_session = publisher_handle.await??;

    // Cold subscriber on relay-B. Publishing is already finished, so the
    // upstream SUBSCRIBE brings no live objects: the published groups exist
    // only in relay-A's cache.
    let cold_session = connect_to_relay(relay_b_url).await?;
    let mut cold_sub = cold_session.subscriber();
    let cold_subscription = cold_sub
        .subscribe(
            namespace.clone(),
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
        "[cold-join] subscribe ok on relay-B, request_id={}",
        cold_subscription.request_id()
    );

    // Joining Fetch 2 groups back from largest (g2): groups 0..=2 in full.
    let received = run_joining_fetch_with_label(
        &mut cold_sub,
        cold_subscription.request_id(),
        2,
        "cold-join",
    )
    .await?;
    let expected: Vec<(u64, u64)> = (0..GROUPS)
        .flat_map(|g| (0..OBJECTS_PER_GROUP).map(move |o| (g, o)))
        .collect();
    assert_eq!(
        received, expected,
        "[H] cold-cache joining fetch must deliver all groups via upstream forwarding"
    );

    // Same joining fetch on a fresh session: served from relay-B's now-warm cache.
    let warm_session = connect_to_relay(relay_b_url).await?;
    let mut warm_sub = warm_session.subscriber();
    let warm_subscription = warm_sub
        .subscribe(
            namespace.clone(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    let received = run_joining_fetch_with_label(
        &mut warm_sub,
        warm_subscription.request_id(),
        2,
        "warm-join",
    )
    .await?;
    assert_eq!(
        received, expected,
        "[H] repeated joining fetch must be served from relay-B's warmed cache"
    );

    drop(publisher_session);
    tracing::info!("[H] OK: cold-cache joining fetch filled via upstream and warmed the cache");
    Ok(())
}

/// Publisher for scenario I: serves one SUBSCRIBE with groups 0..GROUPS, then
/// holds group `GROUPS` until signalled, so the test controls exactly when the
/// upstream largest advances past the subscribe-time largest.
async fn live_publisher_event_loop(
    session: Session<QUIC>,
    label: &'static str,
    group_signal_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<Session<QUIC>> {
    let handler = loop {
        match session.receive_event().await {
            Ok(moqt::SessionEvent::Subscribe(handler)) => break handler,
            Ok(moqt::SessionEvent::Disconnected()) => {
                anyhow::bail!("[{}] disconnected before Subscribe", label)
            }
            Ok(_) => {}
            Err(e) => anyhow::bail!("[{}] session error: {}", label, e),
        }
    };
    tracing::info!("[{}] received Subscribe", label);
    let track_alias = handler.ok(0, ContentExists::False).await?;
    let publication = handler.into_subscription(track_alias);
    let factory = session.publisher().create_stream(&publication);
    for group_id in 0..GROUPS {
        send_group(&factory, group_id).await?;
    }
    tracing::info!(
        "[{}] groups 0..={} published, holding group {}",
        label,
        GROUPS - 1,
        GROUPS
    );
    group_signal_rx
        .await
        .map_err(|_| anyhow::anyhow!("[{}] group-{} signal sender dropped", label, GROUPS))?;
    send_group(&factory, GROUPS).await?;
    tracing::info!("[{}] group {} published", label, GROUPS);
    Ok(session)
}

/// Cross-relay scenario I: joining fetch / live subscription contiguity while
/// the publisher is still publishing. The subscriber joins relay-B (cold) when
/// the largest is g2; the publisher then publishes g3, advancing the upstream
/// largest, before the Joining Fetch is issued. The fetch's absolute range must
/// stay pinned at the subscribe-time largest (draft-14 §9.16.2.1): exactly
/// groups 0..=2, no g3 objects — g3 arrives via the live subscription instead,
/// so fetch + live cover g0..=g3 with no gap at the seam.
async fn run_cross_relay_live_joining_fetch(
    relay_a_url: &str,
    relay_b_url: &str,
) -> anyhow::Result<()> {
    tracing::info!("[I] cross-relay live joining FETCH: largest advances before the fetch");

    let namespace = unique_namespace("live-join-fetch");
    let track_name = "data";

    let publisher_session = connect_to_relay(relay_a_url).await?;
    publisher_session
        .publisher()
        .publish_namespace(namespace.clone())
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (group3_tx, group3_rx) = tokio::sync::oneshot::channel::<()>();
    let publisher_handle = tokio::spawn(live_publisher_event_loop(
        publisher_session,
        "judy",
        group3_rx,
    ));

    // Local subscriber on relay-A drains groups 0..=2 so relay-A's cache is
    // deterministically populated while the publisher holds group 3.
    let local_sub_session = connect_to_relay(relay_a_url).await?;
    let mut local_sub = local_sub_session.subscriber();
    let local_subscription = local_sub
        .subscribe(
            namespace.clone(),
            track_name.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    drain_until_group_complete(&mut local_sub, &local_subscription, GROUPS - 1, "local-sub")
        .await?;

    // Subscribe on relay-B while the publisher is parked: largest-at-subscribe
    // is deterministically g2.
    let live_session = connect_to_relay(relay_b_url).await?;
    let mut live_sub = live_session.subscriber();
    let live_subscription = live_sub
        .subscribe(
            namespace.clone(),
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
        "[live-join] subscribe ok on relay-B, request_id={}",
        live_subscription.request_id()
    );

    // Release group 3 and wait until the live subscription has it in full, so
    // the upstream largest has definitively advanced past g2 when the joining
    // fetch is issued. We assert "group 3 fully received live" rather than
    // strict fetch/live set-disjointness because the LargestObject filter may
    // also re-deliver the boundary object of group 2 live.
    group3_tx
        .send(())
        .map_err(|_| anyhow::anyhow!("[I] publisher exited before the group-3 signal"))?;
    let publisher_session = publisher_handle.await??;
    drain_until_group_complete(&mut live_sub, &live_subscription, GROUPS, "live-join").await?;

    // Joining Fetch 2 groups back from the subscribe-time largest (g2): exactly
    // groups 0..=2 in full, ending at g2's last object, with no g3 objects even
    // though upstream now has g3.
    let received = run_joining_fetch_with_label(
        &mut live_sub,
        live_subscription.request_id(),
        2,
        "live-join",
    )
    .await?;
    let expected: Vec<(u64, u64)> = (0..GROUPS)
        .flat_map(|g| (0..OBJECTS_PER_GROUP).map(move |o| (g, o)))
        .collect();
    assert_eq!(
        received, expected,
        "[I] joining fetch must stay pinned at the subscribe-time largest (groups 0..=2 only)"
    );

    drop(publisher_session);
    tracing::info!("[I] OK: joining fetch pinned at subscribe-time largest; g3 arrived live");
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
    let local_namespace = unique_namespace("alice");

    let alice_handle = tokio::spawn(alice(local_namespace.clone()));
    let mut bob_handle = tokio::spawn(bob(local_namespace.clone(), carol_ready_tx, carol_done_rx));
    tokio::spawn(carol(local_namespace, carol_ready_rx, carol_done_tx));

    // Success requires Bob's (and, through him, Carol's) assertions to actually
    // run, so always wait for Bob to finish. Alice is a background producer; only
    // surface her early exit if it is an error.
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            r = &mut bob_handle => { r??; }
            r = alice_handle => {
                r??;
                bob_handle.await??;
            }
        }
        anyhow::Ok(())
    })
    .await
    .context("local fetch scenarios timed out")??;

    // Cross-relay scenarios run only when two relay URLs are provided.
    let relay_a_url = env::var("MOQT_E2E_RELAY_A_URL").ok();
    let relay_b_url = env::var("MOQT_E2E_RELAY_B_URL").ok();

    if let (Some(relay_a_url), Some(relay_b_url)) = (relay_a_url, relay_b_url) {
        if relay_a_url != relay_b_url {
            tokio::time::timeout(
                Duration::from_secs(30),
                run_cross_relay_standalone_fetch(&relay_a_url, &relay_b_url),
            )
            .await
            .context("cross-relay scenario D timed out")??;
            tokio::time::timeout(
                Duration::from_secs(30),
                run_cross_relay_joining_fetch(&relay_a_url, &relay_b_url),
            )
            .await
            .context("cross-relay scenario E timed out")??;
            tokio::time::timeout(
                Duration::from_secs(30),
                run_cross_relay_leading_gap_fetch(&relay_a_url, &relay_b_url),
            )
            .await
            .context("cross-relay scenario G timed out")??;
            tokio::time::timeout(
                Duration::from_secs(30),
                run_upstream_fetch_forwarding_data_not_found(&relay_a_url, &relay_b_url),
            )
            .await
            .context("cross-relay scenario F timed out")??;
            tokio::time::timeout(
                Duration::from_secs(30),
                run_cross_relay_cold_cache_joining_fetch(&relay_a_url, &relay_b_url),
            )
            .await
            .context("cross-relay scenario H timed out")??;
            tokio::time::timeout(
                Duration::from_secs(30),
                run_cross_relay_live_joining_fetch(&relay_a_url, &relay_b_url),
            )
            .await
            .context("cross-relay scenario I timed out")??;
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
