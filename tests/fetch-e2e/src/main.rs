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
    start: Location,
    end: Location,
) -> anyhow::Result<Vec<(u64, u64)>> {
    tracing::info!(
        "[bob] fetching g{}:o{}..g{}:o{}",
        start.group_id,
        start.object_id,
        end.group_id,
        end.object_id
    );
    let handle = subscriber
        .fetch(
            NAMESPACE.to_string(),
            TRACK_NAME.to_string(),
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
                tracing::info!("[bob] g{}:o{} = {}", obj.group_id, obj.object_id, payload);
                received.push((obj.group_id, obj.object_id));
            }
            Ok(Fetch::End) => {
                tracing::info!(
                    "[bob] fetch done g{}:o{}..g{}:o{}",
                    start.group_id,
                    start.object_id,
                    end.group_id,
                    end.object_id
                );
                break;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "[bob] fetch g{}:o{}..g{}:o{} failed: {}",
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
                Ok(Some(Subgroup::Header(h))) => {
                    tracing::info!("[bob] live group_id={}", h.group_id);
                    if h.group_id >= 2 {
                        tracing::info!("[bob] group 2 detected, relay cache ready");
                        break 'detect;
                    }
                }
                Ok(Some(Subgroup::Object(_))) => {}
                Ok(None) | Err(_) => break, // stream ended, move to next group
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

    Ok(())
}
