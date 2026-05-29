use std::{
    future::Future,
    net::{SocketAddr, ToSocketAddrs},
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{Context as _, bail};
use bytes::Bytes;
use moqt::{
    ClientConfig, ContentExists, DatagramField, Endpoint, ExtensionHeaders, FilterType, GroupOrder,
    QUIC, Session, SessionEvent, Subgroup, SubgroupId,
    SubgroupObject, SubscribeOption, Subscription,
};
use redis::AsyncCommands;

const DEFAULT_RELAY_A_URL: &str = "moqt://localhost:4433";
const DEFAULT_RELAY_B_URL: &str = "moqt://localhost:4434";
const DEFAULT_REDIS_URL: &str = "redis://localhost:6379";
const DEFAULT_TRACK_NAMESPACE: &str = "App/Channel/UserA";
const DEFAULT_TRACK_NAME: &str = "video";
const TEST_PAYLOAD: &[u8] = b"cascading relay e2e payload";
const CATALOG_TRACK_NAME: &str = "catalog";
const CATALOG_PAYLOAD: &[u8] = b"cascading relay e2e catalog payload";
const ORDERED_OBJECT_COUNT: usize = 50;

#[derive(Debug)]
struct Config {
    relay_a_url: String,
    relay_b_url: String,
    redis_url: String,
    track_namespace: String,
    track_name: String,
}

struct PubSubResult {
    _publisher_session: Arc<Session<QUIC>>,
    _subscriber_session: Arc<Session<QUIC>>,
    subscriber_event_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

#[derive(Clone, Copy, Debug)]
enum PublisherSetup {
    PublishNamespaceOnly,
    PublishNamespaceAndTrack,
}

impl Drop for PubSubResult {
    fn drop(&mut self) {
        if let Some(task) = &self.subscriber_event_task {
            task.abort();
        }
    }
}

impl Config {
    fn from_args() -> anyhow::Result<Self> {
        let mut config = Self {
            relay_a_url: DEFAULT_RELAY_A_URL.to_string(),
            relay_b_url: DEFAULT_RELAY_B_URL.to_string(),
            redis_url: DEFAULT_REDIS_URL.to_string(),
            track_namespace: DEFAULT_TRACK_NAMESPACE.to_string(),
            track_name: DEFAULT_TRACK_NAME.to_string(),
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
            match arg.as_str() {
                "--relay-a-url" | "--publisher-url" => config.relay_a_url = value,
                "--relay-b-url" | "--subscriber-url" => config.relay_b_url = value,
                "--redis-url" => config.redis_url = value,
                "--track-namespace" => config.track_namespace = value,
                "--track-name" => config.track_name = value,
                _ => bail!("unknown argument: {arg}"),
            }
        }

        Ok(config)
    }

    fn validate_distinct_relays(&self) -> anyhow::Result<()> {
        let relay_a_address = relay_socket_addr(&self.relay_a_url)?;
        let relay_b_address = relay_socket_addr(&self.relay_b_url)?;
        if relay_a_address == relay_b_address {
            bail!(
                "relay-a and relay-b must use different endpoints: {}",
                relay_a_address
            );
        }

        tracing::info!(
            %relay_a_address,
            %relay_b_address,
            "validated relay endpoints are different"
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,cascading_relay_e2e=info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let config = Config::from_args()?;
    config.validate_distinct_relays()?;
    tracing::info!(?config, "starting cascading relay e2e");

    tokio::time::timeout(Duration::from_secs(90), run(config))
        .await
        .context("timed out running cascading relay e2e")?
}

async fn run(config: Config) -> anyhow::Result<()> {
    let run_id = run_id();

    run_pub_sub_scenario(
        "single relay pub/sub",
        &config.relay_a_url,
        &config.relay_a_url,
        &scenario_namespace(&config.track_namespace, &run_id, "single"),
        &config.track_name,
        None,
        PublisherSetup::PublishNamespaceAndTrack,
    )
    .await?;

    run_pub_sub_scenario(
        "dual relay pub/sub relay-a to relay-b",
        &config.relay_a_url,
        &config.relay_b_url,
        &scenario_namespace(&config.track_namespace, &run_id, "dual-a-to-b"),
        &config.track_name,
        None,
        PublisherSetup::PublishNamespaceAndTrack,
    )
    .await?;

    run_pub_sub_scenario(
        "dual relay pub/sub relay-b to relay-a",
        &config.relay_b_url,
        &config.relay_a_url,
        &scenario_namespace(&config.track_namespace, &run_id, "dual-b-to-a"),
        &config.track_name,
        None,
        PublisherSetup::PublishNamespaceAndTrack,
    )
    .await?;

    run_pub_sub_scenario(
        "dual relay pub/sub caused by end subscriber subscribe",
        &config.relay_a_url,
        &config.relay_b_url,
        &scenario_namespace(&config.track_namespace, &run_id, "subscribe-caused"),
        &config.track_name,
        None,
        PublisherSetup::PublishNamespaceOnly,
    )
    .await?;

    run_namespace_catalog_track_scenario(
        "dual relay namespace -> catalog -> track relay-a to relay-b",
        &config.relay_a_url,
        &config.relay_b_url,
        &scenario_namespace(&config.track_namespace, &run_id, "catalog-track-a-to-b"),
        &config.track_name,
    )
    .await?;

    run_ordered_objects_scenario(
        "dual relay ordered objects relay-a to relay-b",
        &config.relay_a_url,
        &config.relay_b_url,
        &scenario_namespace(&config.track_namespace, &run_id, "ordered-a-to-b"),
        &config.track_name,
        ORDERED_OBJECT_COUNT,
    )
    .await?;

    run_namespace_cleanup_scenario(
        &config.redis_url,
        "relay-b",
        &config.relay_a_url,
        &config.relay_b_url,
        &scenario_namespace(&config.track_namespace, &run_id, "cleanup"),
        &config.track_name,
    )
    .await?;

    tracing::info!("cascading relay e2e passed");
    println!("cascading relay e2e passed");
    Ok(())
}

async fn run_namespace_cleanup_scenario(
    redis_url: &str,
    subscriber_relay_id: &str,
    publisher_url: &str,
    subscriber_url: &str,
    track_namespace: &str,
    track_name: &str,
) -> anyhow::Result<()> {
    tracing::info!(
        %track_namespace,
        %track_name,
        "running dual relay namespace cleanup scenario"
    );
    delete_namespace_subscription(redis_url, track_namespace).await?;

    let result = run_pub_sub_scenario(
        "dual relay pub/sub with subscribe namespace",
        publisher_url,
        subscriber_url,
        track_namespace,
        track_name,
        Some(track_namespace),
        PublisherSetup::PublishNamespaceAndTrack,
    )
    .await?;

    wait_for_namespace_subscription_status(
        redis_url,
        track_namespace,
        subscriber_relay_id,
        Some("active"),
        Duration::from_secs(5),
    )
    .await
    .context("subscriber relay did not register SUBSCRIBE_NAMESPACE route")?;

    tracing::info!(
        relay_id = subscriber_relay_id,
        track_namespace_prefix = %track_namespace,
        "dropping subscriber session and waiting for namespace subscription cleanup"
    );
    drop(result);

    wait_for_namespace_subscription_status(
        redis_url,
        track_namespace,
        subscriber_relay_id,
        None,
        Duration::from_secs(15),
    )
    .await
    .context("subscriber relay did not remove SUBSCRIBE_NAMESPACE route after session close")?;
    Ok(())
}

async fn run_pub_sub_scenario(
    name: &str,
    publisher_url: &str,
    subscriber_url: &str,
    track_namespace: &str,
    track_name: &str,
    subscribe_namespace_prefix: Option<&str>,
    publisher_setup: PublisherSetup,
) -> anyhow::Result<PubSubResult> {
    tracing::info!(
        scenario = name,
        %publisher_url,
        %subscriber_url,
        %track_namespace,
        %track_name,
        ?publisher_setup,
        "running pub/sub scenario"
    );

    let publisher_session = Arc::new(connect_with_retry(publisher_url).await?);
    let publisher = publisher_session.publisher();
    publisher
        .publish_namespace(track_namespace.to_string())
        .await
        .context("publisher failed to publish namespace")?;
    let published_resource = if let PublisherSetup::PublishNamespaceAndTrack = publisher_setup {
        let subscription = publisher
            .publish(
                track_namespace.to_string(),
                track_name.to_string(),
                moqt::PublishOption::default(),
            )
            .await
            .context("publisher failed to publish track")?;
        Some(subscription)
    } else {
        None
    };

    let publisher_task = if published_resource.is_some() {
        None
    } else {
        Some(spawn_publisher_event_loop(
            publisher_session.clone(),
            TEST_PAYLOAD,
        ))
    };

    tokio::time::sleep(Duration::from_millis(500)).await;

    let subscriber_session = Arc::new(connect_with_retry(subscriber_url).await?);
    let subscriber_event_task = if subscribe_namespace_prefix.is_some() {
        Some(spawn_subscriber_namespace_event_loop(
            subscriber_session.clone(),
        ))
    } else {
        None
    };

    if let Some(prefix) = subscribe_namespace_prefix {
        tracing::info!(track_namespace_prefix = %prefix, "subscriber sending SUBSCRIBE_NAMESPACE");
        subscriber_session
            .subscriber()
            .subscribe_namespace(prefix.to_string())
            .await
            .context("subscriber failed to subscribe namespace")?;
    }

    let received = if let Some(published_resource) = published_resource {
        let publisher_session = publisher_session.clone();
        subscribe_and_receive_one_object_after_subscribe(
            subscriber_session.clone(),
            track_namespace.to_string(),
            track_name.to_string(),
            move || send_test_object(publisher_session, published_resource, TEST_PAYLOAD),
        )
        .await?
    } else {
        subscribe_and_receive_one_object(
            subscriber_session.clone(),
            track_namespace.to_string(),
            track_name.to_string(),
        )
        .await?
    };

    if let Some(publisher_task) = publisher_task {
        publisher_task.await??;
    }
    if received != TEST_PAYLOAD {
        bail!(
            "unexpected payload in {name}: expected {:?}, got {:?}",
            TEST_PAYLOAD,
            received
        );
    }

    tracing::info!(scenario = name, "pub/sub scenario passed");
    Ok(PubSubResult {
        _publisher_session: publisher_session,
        _subscriber_session: subscriber_session,
        subscriber_event_task,
    })
}

/// Exercises the realistic call flow end to end across two relays:
/// publisher PUBLISH_NAMESPACE on relay-a, subscriber SUBSCRIBE_NAMESPACE on
/// relay-b (and observes the resulting PUBLISH_NAMESPACE), then SUBSCRIBE the
/// `catalog` track and a media track, verifying the right object arrives on each.
async fn run_namespace_catalog_track_scenario(
    name: &str,
    publisher_url: &str,
    subscriber_url: &str,
    track_namespace: &str,
    media_track_name: &str,
) -> anyhow::Result<()> {
    tracing::info!(
        scenario = name,
        %publisher_url,
        %subscriber_url,
        %track_namespace,
        catalog_track_name = CATALOG_TRACK_NAME,
        %media_track_name,
        "running namespace -> catalog -> track scenario"
    );

    // Publisher: announce the namespace and answer SUBSCRIBE for catalog + media.
    let publisher_session = Arc::new(connect_with_retry(publisher_url).await?);
    publisher_session
        .publisher()
        .publish_namespace(track_namespace.to_string())
        .await
        .context("publisher failed to publish namespace")?;
    let publisher_task = spawn_catalog_track_publisher_loop(
        publisher_session.clone(),
        media_track_name.to_string(),
    );

    // Give the namespace route time to propagate to the subscriber relay.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Subscriber: SUBSCRIBE_NAMESPACE and wait for the matching PUBLISH_NAMESPACE.
    let subscriber_session = Arc::new(connect_with_retry(subscriber_url).await?);
    let (discovered_tx, mut discovered_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let subscriber_task = spawn_subscriber_namespace_discovery_loop(
        subscriber_session.clone(),
        discovered_tx,
    );

    tracing::info!(track_namespace_prefix = %track_namespace, "subscriber sending SUBSCRIBE_NAMESPACE");
    subscriber_session
        .subscriber()
        .subscribe_namespace(track_namespace.to_string())
        .await
        .context("subscriber failed to subscribe namespace")?;

    let discovered = tokio::time::timeout(Duration::from_secs(15), discovered_rx.recv())
        .await
        .context("timed out waiting for PUBLISH_NAMESPACE")?
        .context("subscriber namespace discovery loop closed before PUBLISH_NAMESPACE")?;
    if !discovered.starts_with(track_namespace) {
        bail!(
            "discovered namespace {discovered:?} does not match expected prefix {track_namespace:?}"
        );
    }
    tracing::info!(%discovered, "subscriber discovered namespace via PUBLISH_NAMESPACE");

    // SUBSCRIBE the catalog track and verify its object.
    let catalog = subscribe_and_receive_one_object(
        subscriber_session.clone(),
        track_namespace.to_string(),
        CATALOG_TRACK_NAME.to_string(),
    )
    .await
    .context("subscriber failed to receive catalog object")?;
    if catalog != CATALOG_PAYLOAD {
        bail!(
            "unexpected catalog payload in {name}: expected {:?}, got {:?}",
            CATALOG_PAYLOAD,
            catalog
        );
    }
    tracing::info!("subscriber received expected catalog object");

    // SUBSCRIBE the media track and verify its object.
    let media = subscribe_and_receive_one_object(
        subscriber_session.clone(),
        track_namespace.to_string(),
        media_track_name.to_string(),
    )
    .await
    .context("subscriber failed to receive track object")?;
    if media != TEST_PAYLOAD {
        bail!(
            "unexpected track payload in {name}: expected {:?}, got {:?}",
            TEST_PAYLOAD,
            media
        );
    }
    tracing::info!("subscriber received expected track object");

    publisher_task.abort();
    subscriber_task.abort();
    tracing::info!(scenario = name, "namespace -> catalog -> track scenario passed");
    Ok(())
}

/// Publisher event loop that answers SUBSCRIBE for the catalog and media tracks,
/// sending a distinct payload on each so the subscriber can tell them apart.
fn spawn_catalog_track_publisher_loop(
    session: Arc<Session<QUIC>>,
    media_track_name: String,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        loop {
            let event = session.receive_event().await?;
            match event {
                SessionEvent::Subscribe(handler) => {
                    let track_name = handler.track_name.clone();
                    let payload: &'static [u8] = if track_name == CATALOG_TRACK_NAME {
                        CATALOG_PAYLOAD
                    } else if track_name == media_track_name {
                        TEST_PAYLOAD
                    } else {
                        tracing::warn!(%track_name, "publisher ignoring subscribe for unexpected track");
                        continue;
                    };
                    tracing::info!(
                        track_namespace = %handler.track_namespace,
                        %track_name,
                        "publisher received subscribe; responding with object"
                    );
                    // `ok()` allocates and returns the track_alias for this track;
                    // send objects on that same alias so the relay can route them.
                    // Hardcoding 0 collides once a second track is subscribed.
                    let track_alias = handler.ok(1_000_000, ContentExists::False).await?;
                    let publication = handler.into_subscriber_initiated_subscription(track_alias);
                    send_test_object(session.clone(), publication, payload).await?;
                }
                SessionEvent::ProtocolViolation() => bail!("publisher protocol violation"),
                SessionEvent::Disconnected() => return Ok(()),
                _ => {}
            }
        }
    })
}

/// Subscriber event loop that ACKs PUBLISH_NAMESPACE and reports each discovered
/// namespace back to the scenario through `discovered_tx`.
fn spawn_subscriber_namespace_discovery_loop(
    session: Arc<Session<QUIC>>,
    discovered_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        loop {
            let event = session.receive_event().await?;
            match event {
                SessionEvent::PublishNamespace(handler) => {
                    let namespace = handler.track_namespace.clone();
                    tracing::info!(
                        track_namespace = %namespace,
                        "subscriber received PUBLISH_NAMESPACE"
                    );
                    handler.ok().await?;
                    let _ = discovered_tx.send(namespace);
                }
                SessionEvent::ProtocolViolation() => bail!("subscriber protocol violation"),
                SessionEvent::Disconnected() => return Ok(()),
                _ => {}
            }
        }
    })
}

/// Publisher sends `count` objects in order on a single subgroup stream, then the
/// subscriber reads them back and the scenario verifies they arrive in the same
/// order with no gaps or reordering. Runs across two relays (relay-a -> relay-b).
async fn run_ordered_objects_scenario(
    name: &str,
    publisher_url: &str,
    subscriber_url: &str,
    track_namespace: &str,
    track_name: &str,
    count: usize,
) -> anyhow::Result<()> {
    tracing::info!(
        scenario = name,
        %publisher_url,
        %subscriber_url,
        %track_namespace,
        %track_name,
        count,
        "running ordered objects scenario"
    );

    let publisher_session = Arc::new(connect_with_retry(publisher_url).await?);
    publisher_session
        .publisher()
        .publish_namespace(track_namespace.to_string())
        .await
        .context("publisher failed to publish namespace")?;
    let publisher_task = spawn_ordered_objects_publisher_loop(publisher_session.clone(), count);

    // Give the namespace route time to propagate to the subscriber relay.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let subscriber_session = Arc::new(connect_with_retry(subscriber_url).await?);
    let received = tokio::time::timeout(
        Duration::from_secs(30),
        subscribe_and_receive_ordered_objects(
            subscriber_session.clone(),
            track_namespace.to_string(),
            track_name.to_string(),
            count,
        ),
    )
    .await
    .context("timed out waiting for ordered objects")??;

    publisher_task.abort();

    if received.len() != count {
        bail!(
            "expected {count} objects in {name}, got {}",
            received.len()
        );
    }
    for (index, payload) in received.iter().enumerate() {
        let expected = ordered_object_payload(index);
        if payload != &expected {
            bail!(
                "ordered objects out of order in {name} at index {index}: expected {:?}, got {:?}",
                String::from_utf8_lossy(&expected),
                String::from_utf8_lossy(payload)
            );
        }
    }

    tracing::info!(scenario = name, count, "ordered objects scenario passed (received in order)");
    Ok(())
}

/// Publisher event loop that answers each SUBSCRIBE by sending `count` ordered
/// objects (payload `ordered-object-{i}`) on a single subgroup stream.
fn spawn_ordered_objects_publisher_loop(
    session: Arc<Session<QUIC>>,
    count: usize,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        loop {
            match session.receive_event().await? {
                SessionEvent::Subscribe(handler) => {
                    tracing::info!(
                        track_namespace = %handler.track_namespace,
                        track_name = %handler.track_name,
                        count,
                        "publisher received subscribe; sending ordered objects"
                    );
                    let track_alias = handler.ok(1_000_000, ContentExists::False).await?;
                    let publication = handler.into_subscriber_initiated_subscription(track_alias);
                    send_ordered_objects(session.clone(), publication, count).await?;
                }
                SessionEvent::ProtocolViolation() => bail!("publisher protocol violation"),
                SessionEvent::Disconnected() => return Ok(()),
                _ => {}
            }
        }
    })
}

/// Sends `count` objects on one subgroup stream with sequential object ids
/// (delta 0 for the first object, 1 for each subsequent one).
async fn send_ordered_objects(
    session: Arc<Session<QUIC>>,
    publication: Subscription,
    count: usize,
) -> anyhow::Result<()> {
    let stream_factory = session.publisher().create_stream(&publication);
    let uninitialized = stream_factory.next().await?;
    let header = uninitialized.create_header(0, SubgroupId::None, 128, false, false);
    let mut stream = uninitialized.send_header(header).await?;
    for index in 0..count {
        let object_id_delta = if index == 0 { 0 } else { 1 };
        let object = stream.create_object_field(
            object_id_delta,
            ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            SubgroupObject::new_payload(Bytes::from(ordered_object_payload(index))),
        );
        stream.send(object).await?;
    }
    stream.close().await?;
    tracing::info!(count, "publisher sent ordered objects");
    Ok(())
}

/// Subscribes and reads exactly `count` payload objects in arrival order.
async fn subscribe_and_receive_ordered_objects(
    session: Arc<Session<QUIC>>,
    track_namespace: String,
    track_name: String,
    count: usize,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let option = SubscribeOption {
        subscriber_priority: 128,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::LargestObject,
    };
    tracing::info!(%track_namespace, %track_name, count, "subscriber sending SUBSCRIBE for ordered objects");
    let subscription = session
        .subscriber()
        .subscribe(track_namespace, track_name, option)
        .await
        .context("subscriber failed to subscribe")?;
    let receiver = session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
        .context("subscriber failed to accept data receiver")?;

    let mut payloads = Vec::with_capacity(count);
    match receiver {
        moqt::DataReceiver::Stream(mut factory) => {
            let mut stream = factory.next().await?;
            while payloads.len() < count {
                if let Subgroup::Object(field) = stream.receive().await?
                    && let SubgroupObject::Payload { data, .. } = field.subgroup_object
                {
                    payloads.push(data.to_vec());
                }
            }
        }
        moqt::DataReceiver::Datagram(_) => {
            bail!("expected stream data receiver for ordered objects");
        }
    }
    tracing::info!(received = payloads.len(), "subscriber received ordered objects");
    Ok(payloads)
}

fn ordered_object_payload(index: usize) -> Vec<u8> {
    format!("ordered-object-{index}").into_bytes()
}

async fn connect(url: &str) -> anyhow::Result<Session<QUIC>> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
    })?;
    let url = url::Url::from_str(url)?;
    let host = url.host_str().context("missing host")?;
    let remote_address = relay_socket_addr(url.as_str())?;

    tracing::info!(%remote_address, host, "connecting to relay");
    let connecting = endpoint.connect(remote_address, host).await?;
    connecting.await
}

fn relay_socket_addr(url: &str) -> anyhow::Result<SocketAddr> {
    let url = url::Url::from_str(url)?;
    let host = url.host_str().context("missing host")?;
    let port = url.port().unwrap_or(443);
    (host, port)
        .to_socket_addrs()?
        .next()
        .context("failed to resolve relay address")
}

async fn connect_with_retry(url: &str) -> anyhow::Result<Session<QUIC>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last_error = None;

    while tokio::time::Instant::now() < deadline {
        match connect(url).await {
            Ok(session) => return Ok(session),
            Err(err) => {
                tracing::info!(?err, url, "relay is not ready yet");
                last_error = Some(err);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("failed to connect to {url}")))
}

fn spawn_publisher_event_loop(
    session: Arc<Session<QUIC>>,
    payload: &'static [u8],
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        loop {
            let event = session.receive_event().await?;
            match event {
                SessionEvent::Subscribe(handler) => {
                    tracing::info!(
                        track_namespace = %handler.track_namespace,
                        track_name = %handler.track_name,
                        "publisher received subscribe"
                    );
                    handler.ok(1_000_000, ContentExists::False).await?;
                    let publication = handler.into_subscriber_initiated_subscription(0);
                    send_test_object(session.clone(), publication, payload).await?;
                    return Ok(());
                }
                SessionEvent::ProtocolViolation() => bail!("publisher protocol violation"),
                SessionEvent::Disconnected() => bail!("publisher disconnected"),
                _ => {}
            }
        }
    })
}

async fn send_test_object(
    session: Arc<Session<QUIC>>,
    publication: Subscription,
    payload: &'static [u8],
) -> anyhow::Result<()> {
    let stream_factory = session.publisher().create_stream(&publication);
    let uninitialized = stream_factory.next().await?;
    let header = uninitialized.create_header(0, SubgroupId::None, 128, false, false);
    let mut stream = uninitialized.send_header(header).await?;
    let object = stream.create_object_field(
        0,
        ExtensionHeaders {
            prior_group_id_gap: vec![],
            prior_object_id_gap: vec![],
            immutable_extensions: vec![],
        },
        SubgroupObject::new_payload(Bytes::from_static(payload)),
    );
    stream.send(object).await?;
    stream.close().await?;
    tracing::info!("publisher sent test object");
    Ok(())
}

fn spawn_subscriber_namespace_event_loop(
    session: Arc<Session<QUIC>>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        loop {
            let event = session.receive_event().await?;
            match event {
                SessionEvent::PublishNamespace(handler) => {
                    tracing::info!(
                        track_namespace = %handler.track_namespace,
                        "subscriber received PUBLISH_NAMESPACE"
                    );
                    handler.ok().await?;
                    tracing::info!(
                        track_namespace = %handler.track_namespace,
                        "subscriber sent PUBLISH_NAMESPACE_OK"
                    );
                }
                SessionEvent::ProtocolViolation() => bail!("subscriber protocol violation"),
                SessionEvent::Disconnected() => return Ok(()),
                _ => {}
            }
        }
    })
}

async fn subscribe_and_receive_one_object(
    session: Arc<Session<QUIC>>,
    track_namespace: String,
    track_name: String,
) -> anyhow::Result<Vec<u8>> {
    subscribe_and_receive_one_object_after_subscribe(
        session,
        track_namespace,
        track_name,
        || async { Ok(()) },
    )
    .await
}

async fn subscribe_and_receive_one_object_after_subscribe<F, Fut>(
    session: Arc<Session<QUIC>>,
    track_namespace: String,
    track_name: String,
    after_subscribe: F,
) -> anyhow::Result<Vec<u8>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    tokio::time::timeout(
        Duration::from_secs(15),
        subscribe_and_receive_one_object_inner(
            session,
            track_namespace,
            track_name,
            after_subscribe,
        ),
    )
    .await
    .context("timed out waiting for subscriber object")?
}

async fn subscribe_and_receive_one_object_inner<F, Fut>(
    session: Arc<Session<QUIC>>,
    track_namespace: String,
    track_name: String,
    after_subscribe: F,
) -> anyhow::Result<Vec<u8>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let option = SubscribeOption {
        subscriber_priority: 128,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::LargestObject,
    };
    tracing::info!(
        %track_namespace,
        %track_name,
        "subscriber sending SUBSCRIBE"
    );
    let subscription = session
        .subscriber()
        .subscribe(track_namespace, track_name, option)
        .await
        .context("subscriber failed to subscribe")?;
    tracing::info!(
        track_alias = subscription.track_alias(),
        "subscriber received SUBSCRIBE_OK; accepting data receiver"
    );
    // Act: downstream subscription の準備ができてから publisher にテスト object を送らせる。
    after_subscribe().await?;
    let receiver = session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
        .context("subscriber failed to accept data receiver")?;
    tracing::info!("subscriber accepted data receiver; waiting for object payload");

    match receiver {
        moqt::DataReceiver::Stream(mut factory) => {
            let mut stream = factory.next().await?;
            loop {
                if let Subgroup::Object(field) = stream.receive().await?
                    && let SubgroupObject::Payload { data, .. } = field.subgroup_object
                {
                    tracing::info!(
                        payload_len = data.len(),
                        "subscriber received stream object"
                    );
                    return Ok(data.to_vec());
                }
            }
        }
        moqt::DataReceiver::Datagram(mut datagram) => loop {
            let object = datagram.receive().await?;
            match object.field {
                DatagramField::Payload0x00 { payload, .. }
                | DatagramField::Payload0x01 { payload, .. }
                | DatagramField::Payload0x02WithEndOfGroup { payload, .. }
                | DatagramField::Payload0x03WithEndOfGroup { payload, .. }
                | DatagramField::Payload0x04 { payload, .. }
                | DatagramField::Payload0x05 { payload, .. }
                | DatagramField::Payload0x06WithEndOfGroup { payload, .. }
                | DatagramField::Payload0x07WithEndOfGroup { payload, .. } => {
                    tracing::info!(
                        payload_len = payload.len(),
                        "subscriber received datagram object"
                    );
                    return Ok(payload.to_vec());
                }
                DatagramField::Status0x20 { .. } | DatagramField::Status0x21 { .. } => {}
            }
        },
    }
}

async fn delete_namespace_subscription(redis_url: &str, prefix: &str) -> anyhow::Result<()> {
    let mut connection = redis_connection(redis_url).await?;
    let _: () = connection.del(namespace_subscription_key(prefix)).await?;
    Ok(())
}

async fn wait_for_namespace_subscription_status(
    redis_url: &str,
    prefix: &str,
    relay_id: &str,
    expected: Option<&str>,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        let status = namespace_subscription_status(redis_url, prefix, relay_id).await?;
        if status.as_deref() == expected {
            tracing::info!(
                track_namespace_prefix = %prefix,
                relay_id,
                ?expected,
                "observed namespace subscription status"
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let actual = namespace_subscription_status(redis_url, prefix, relay_id).await?;
    bail!(
        "namespace subscription status mismatch for prefix={prefix} relay_id={relay_id}: expected {:?}, got {:?}",
        expected,
        actual
    )
}

async fn namespace_subscription_status(
    redis_url: &str,
    prefix: &str,
    relay_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut connection = redis_connection(redis_url).await?;
    let status: Option<String> = connection
        .hget(namespace_subscription_key(prefix), relay_id)
        .await?;
    Ok(status)
}

async fn redis_connection(redis_url: &str) -> anyhow::Result<redis::aio::ConnectionManager> {
    let client = redis::Client::open(redis_url)?;
    Ok(redis::aio::ConnectionManager::new(client).await?)
}

fn namespace_subscription_key(prefix: &str) -> String {
    format!("route:namespace_subscription:{prefix}")
}

fn scenario_namespace(base: &str, run_id: &str, scenario: &str) -> String {
    format!("{base}/{run_id}/{scenario}")
}

fn run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("run-{millis}")
}
