//! Manual e2e for per-object dedup when the same track is published twice.
//!
//!   1. alice publishes group 0 (o0..o4) on a track, then disconnects.
//!   2. alice publishes the same Full Track Name and group 0 again.
//!   3. bob FETCHes the whole of group 0 and must receive exactly o0..o4.
//!
//! Without dedup the relay appends the second group 0 after the first, so the
//! fetch returns 10 objects (each object id 0..4 twice) and the assertion fails.
//!
//! Run a relay on localhost:4433, then `cargo run -p object-dedup-e2e`.

use std::net::ToSocketAddrs;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moqt::{
    ClientConfig, Endpoint, ExtensionHeaders, Fetch, FetchDataReceiver, FetchObject, FetchOption,
    Location, PublishOption, QUIC, Session, StreamDataSenderFactory, SubgroupId, SubgroupObject,
};

const RELAY_URL: &str = "moqt://localhost:4433";
const NAMESPACE: &str = "room/main";
const PUBLISHER_PRIORITY: u8 = 128;
const GROUP_ID: u64 = 0;
const OBJECTS_PER_GROUP: u64 = 5;

async fn new_session() -> anyhow::Result<Session<QUIC>> {
    let url = url::Url::from_str(RELAY_URL).unwrap();
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

/// A fresh track name per run so a relay's persistent cache (it never evicts)
/// is not polluted by objects from earlier runs of this test.
fn unique_track_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("data-{}", nanos)
}

async fn send_group(factory: &StreamDataSenderFactory<QUIC>, attempt: &str) -> anyhow::Result<()> {
    let sender = factory.next().await?;
    let header = sender.create_header(GROUP_ID, SubgroupId::None, PUBLISHER_PRIORITY, false, false);
    let mut stream = sender.send_header(header).await?;
    for obj_id in 0..OBJECTS_PER_GROUP {
        // Tag the payload with the publish attempt so the duplication is visible.
        let payload = format!("alice:{}:g{}:o{}", attempt, GROUP_ID, obj_id);
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
        tracing::info!("[alice:{}] sent g{}:o{}", attempt, GROUP_ID, obj_id);
    }
    stream.close().await
}

/// Connects, PUBLISHes the track, sends group 0, then drops the session
/// (disconnect). The trailing sleep lets the relay ingest the objects and, on
/// return, tear the session down before the next publish.
async fn publish_once(track_name: &str, attempt: &str) -> anyhow::Result<()> {
    let session = new_session().await?;
    let publisher = session.publisher();
    let subscription = publisher
        .publish(
            NAMESPACE.to_string(),
            track_name.to_string(),
            PublishOption::default(),
        )
        .await?;
    tracing::info!("[alice:{}] publish ok", attempt);
    let factory = publisher.create_stream(&subscription);
    send_group(&factory, attempt).await?;
    tracing::info!(
        "[alice:{}] group sent; waiting for relay to ingest",
        attempt
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// Runs one standalone FETCH and returns the received (group_id, object_id,
/// payload) in delivery order.
async fn run_fetch(
    subscriber: &mut moqt::Subscriber<QUIC>,
    track_name: &str,
    start: Location,
    end: Location,
) -> anyhow::Result<Vec<(u64, u64, String)>> {
    let handle = subscriber
        .fetch(
            NAMESPACE.to_string(),
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
                tracing::info!("[bob] g{}:o{} = {}", obj.group_id, obj.object_id, payload);
                received.push((obj.group_id, obj.object_id, payload));
            }
            Ok(Fetch::End) => break,
            Err(e) => return Err(anyhow::anyhow!("[bob] fetch failed: {}", e)),
        }
    }
    Ok(received)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    let track_name = unique_track_name();
    tracing::info!("[main] track = {}/{}", NAMESPACE, track_name);

    // 1. alice publishes group 0, then disconnects.
    publish_once(&track_name, "1st").await?;
    // Let the relay finish tearing down alice's ingress so the single-writer
    // guard is cleared before the same track is published again.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 2. alice publishes the *same* group 0 again.
    publish_once(&track_name, "2nd").await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 3. bob fetches the whole of group 0. End object_id 0 means "entire group",
    // so the range is not clamped and every cached object is returned.
    let session = new_session().await?;
    let mut subscriber = session.subscriber();
    let received = run_fetch(
        &mut subscriber,
        &track_name,
        Location {
            group_id: GROUP_ID,
            object_id: 0,
        },
        Location {
            group_id: GROUP_ID,
            object_id: 0,
        },
    )
    .await?;

    let ids: Vec<(u64, u64)> = received.iter().map(|(g, o, _)| (*g, *o)).collect();
    let expected: Vec<(u64, u64)> = (0..OBJECTS_PER_GROUP).map(|o| (GROUP_ID, o)).collect();
    tracing::info!("[bob] received {} objects: {:?}", received.len(), received);

    // The relay must deduplicate by (Full Track Name, Group ID, Object ID):
    // publishing the same group 0 twice must not duplicate it.
    assert_eq!(
        ids,
        expected,
        "group 0 must contain exactly o0..o4 after being published twice; got {} objects: {:?}",
        received.len(),
        received
    );

    tracing::info!("[bob] OK: group 0 deduplicated to o0..o4");
    Ok(())
}
