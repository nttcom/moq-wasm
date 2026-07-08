//! E2E for the relay's QUIC stream prioritization (draft-14 §7.2).
//!
//! The relay maps MoQT subscriber/publisher priorities onto QUIC send stream
//! priorities, so when multiple subgroup streams compete for the same
//! connection, the higher-priority stream's bytes are transmitted first.
//!
//! Both scenarios stack the deck AGAINST prioritization and then assert that
//! the higher-priority track is still received faster:
//!
//!   1. Bob subscribes to the low-priority track first, then the
//!      high-priority one, on a single connection.
//!   2. Alice publishes the low-priority track's whole group (16 MiB) first,
//!      then the high-priority one, while Bob is not reading yet. Beyond
//!      Bob's per-stream flow-control window (~1.25 MiB) everything backlogs
//!      at the relay.
//!   3. Bob reads both streams concurrently and records every object's
//!      arrival time; each track's reception speed is its mean arrival time.
//!
//! Without priority control the low track wins by construction (subscribed,
//! published, and pre-buffered first). With it, the relay must transmit the
//! high track's backlog before the low track's remainder, so the high track
//! is received faster by a wide margin.
//!
//! QUIC stream priorities only order data that competes inside the sender's
//! transport; whenever the relay's egress happens to be CPU-bound instead,
//! the two streams progress near-fairly and a single measurement is noise.
//! Each scenario therefore requires a clear margin (low mean arrival at least
//! 1.3x the high one) and takes a best-of-five vote. Without prioritization
//! the margin is structurally unreachable (the low track leads by design), so
//! the vote converges reliably in both directions.
//!
//! Scenario A differentiates by subscriber priority, scenario B by publisher
//! priority with equal subscriber priorities.
//!
//! Run a relay on localhost:4433, then `cargo run -p priority-e2e`.

use std::{
    collections::HashMap,
    env,
    net::ToSocketAddrs,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use moqt::{
    ClientConfig, ContentExists, Endpoint, ExtensionHeaders, FilterType, GroupOrder, QUIC, Session,
    StreamDataSenderFactory, Subgroup, SubgroupId, SubgroupObject, SubscribeOption,
};
use tokio::{sync::oneshot, time::Instant};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const GROUP_ID: u64 = 0;
// Few large objects keep the relay's per-object egress work small, so the
// transport (where priorities apply), not the relay tasks, is the bottleneck.
const OBJECT_SIZE: usize = 256 * 1024;
const OBJECTS_PER_GROUP: u64 = 64; // 16 MiB per track
/// The low track's mean arrival must be at least this factor above the high
/// track's. Fair (unprioritized) delivery cannot reach it: the low track
/// structurally leads, so a fair split puts the ratio at or below ~1.0.
const MEAN_ARRIVAL_MARGIN: f64 = 1.3;
const ATTEMPTS_MAX: u32 = 5;
const SUCCESSES_REQUIRED: u32 = 3;

/// One track in a scenario: how Bob subscribes to it and how Alice stamps
/// its subgroup header.
#[derive(Clone)]
struct TrackSpec {
    track_name: &'static str,
    subscriber_priority: u8,
    publisher_priority: u8,
}

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

/// A fresh namespace per scenario so the relay's persistent cache is not
/// polluted by earlier runs.
fn unique_namespace(scenario: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("priority/{}/{}", scenario, nanos)
}

async fn send_group(
    factory: &StreamDataSenderFactory<QUIC>,
    publisher_priority: u8,
) -> anyhow::Result<()> {
    let sender = factory.next().await?;
    let header = sender.create_header(GROUP_ID, SubgroupId::None, publisher_priority, false, false);
    let mut stream = sender.send_header(header).await?;
    for _ in 0..OBJECTS_PER_GROUP {
        let obj = stream.create_object_field(
            0,
            ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            SubgroupObject::new_payload(vec![0xAB; OBJECT_SIZE].into()),
        );
        stream.send(obj).await?;
    }
    stream.close().await
}

/// Announces the namespace, answers Bob's subscribes, and once every track in
/// `specs` is subscribed publishes one 8 MiB group per track — in `specs`
/// order — before signalling `done_sender`. Bob is not reading yet, so the
/// groups pile up in the relay's egress backlog.
async fn publisher_task(
    namespace: String,
    specs: Vec<TrackSpec>,
    done_sender: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let session = new_session().await?;
    session.publisher().publish_namespace(namespace).await?;

    let mut factories = HashMap::<String, StreamDataSenderFactory<QUIC>>::new();
    while factories.len() < specs.len() {
        match session.receive_event().await? {
            moqt::SessionEvent::Subscribe(handler) => {
                let track_alias = handler.ok(0, ContentExists::False).await?;
                let publication = handler.into_subscription(track_alias);
                let track_name = publication.track_name().to_string();
                let factory = session.publisher().create_stream(&publication);
                factories.insert(track_name, factory);
            }
            moqt::SessionEvent::Disconnected() => {
                anyhow::bail!("publisher disconnected before all subscribes arrived")
            }
            _ => {}
        }
    }

    for spec in &specs {
        let factory = factories
            .get(spec.track_name)
            .expect("factory exists for every subscribed track");
        send_group(factory, spec.publisher_priority).await?;
        tracing::info!("[alice] track {} fully published", spec.track_name);
    }
    let _ = done_sender.send(());

    // Keep the session (and the relay-side track cache) alive while Bob reads.
    loop {
        if let moqt::SessionEvent::Disconnected() = session.receive_event().await? {
            return Ok(());
        }
    }
}

/// How fast one track's objects arrived, measured from the shared read start.
struct TrackReception {
    /// Arrival time of each object averaged over the whole group. Robust
    /// against scheduling noise around any single object, so this is what the
    /// scenarios assert on.
    mean_arrival: Duration,
    last_arrival: Duration,
}

/// Reads the track's single subgroup stream to the end and measures each
/// object's arrival time relative to `started_at`.
async fn read_track(
    session: &Session<QUIC>,
    subscription: &moqt::Subscription,
    track_name: &'static str,
    started_at: Instant,
) -> anyhow::Result<TrackReception> {
    let receiver = session
        .subscriber()
        .accept_data_receiver(subscription)
        .await?;
    let moqt::DataReceiver::Stream(mut factory) = receiver else {
        anyhow::bail!("[bob] expected a stream data receiver for {}", track_name);
    };
    let mut stream = factory.next().await?;
    let mut arrivals = Vec::with_capacity(OBJECTS_PER_GROUP as usize);
    while arrivals.len() < OBJECTS_PER_GROUP as usize {
        if let Subgroup::Object(field) = stream.receive().await? {
            let SubgroupObject::Payload { data, .. } = field.subgroup_object else {
                anyhow::bail!("[bob] unexpected non-payload object on {}", track_name);
            };
            anyhow::ensure!(
                data.len() == OBJECT_SIZE,
                "[bob] wrong payload size on {}: {}",
                track_name,
                data.len()
            );
            arrivals.push(started_at.elapsed());
        }
    }
    let mean_arrival = arrivals.iter().sum::<Duration>() / arrivals.len() as u32;
    let last_arrival = *arrivals.last().expect("group is never empty");
    tracing::info!(
        "[bob] track {} fully received (mean {:?}, last {:?})",
        track_name,
        mean_arrival,
        last_arrival
    );
    Ok(TrackReception {
        mean_arrival,
        last_arrival,
    })
}

/// Runs one scenario and returns `(low_reception, high_reception)`.
/// `low` is the track expected to be received SLOWER; it gets every
/// non-priority advantage (subscribed first, published first).
async fn run_case(
    scenario: &'static str,
    low: TrackSpec,
    high: TrackSpec,
) -> anyhow::Result<(TrackReception, TrackReception)> {
    let namespace = unique_namespace(scenario);
    let (done_sender, done_receiver) = oneshot::channel::<()>();
    let publisher = tokio::spawn(publisher_task(
        namespace.clone(),
        vec![low.clone(), high.clone()],
        done_sender,
    ));
    // Let the relay register the namespace before subscribing.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let session = new_session().await?;
    let mut subscriber = session.subscriber();
    let mut subscriptions = Vec::new();
    for spec in [&low, &high] {
        let subscription = subscriber
            .subscribe(
                namespace.clone(),
                spec.track_name.to_string(),
                SubscribeOption {
                    subscriber_priority: spec.subscriber_priority,
                    group_order: GroupOrder::Ascending,
                    forward: true,
                    filter_type: FilterType::LargestObject,
                },
            )
            .await?;
        subscriptions.push(subscription);
    }

    // Wait until Alice finished publishing so both tracks are fully backlogged
    // at the relay, then start reading both streams at the same time.
    done_receiver.await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started_at = Instant::now();
    let (low_reception, high_reception) = tokio::try_join!(
        read_track(&session, &subscriptions[0], low.track_name, started_at),
        read_track(&session, &subscriptions[1], high.track_name, started_at),
    )?;
    tracing::info!(
        "[{}] high {}: mean {:?} / last {:?}, low {}: mean {:?} / last {:?}",
        scenario,
        high.track_name,
        high_reception.mean_arrival,
        high_reception.last_arrival,
        low.track_name,
        low_reception.mean_arrival,
        low_reception.last_arrival,
    );
    publisher.abort();
    Ok((low_reception, high_reception))
}

/// Repeats the scenario until the margin is met `SUCCESSES_REQUIRED` times
/// (pass) or can no longer be met within `ATTEMPTS_MAX` attempts (fail).
/// Transport/protocol errors abort immediately; only the timing vote retries.
async fn run_scenario_with_vote(
    scenario: &'static str,
    low: TrackSpec,
    high: TrackSpec,
) -> anyhow::Result<()> {
    let mut successes = 0u32;
    for attempt in 1..=ATTEMPTS_MAX {
        let (low_reception, high_reception) = run_case(scenario, low.clone(), high.clone()).await?;
        let ratio =
            low_reception.mean_arrival.as_secs_f64() / high_reception.mean_arrival.as_secs_f64();
        if ratio >= MEAN_ARRIVAL_MARGIN {
            successes += 1;
        }
        tracing::info!(
            "[{}] attempt {}: low/high mean arrival ratio {:.2} ({} of {} required successes)",
            scenario,
            attempt,
            ratio,
            successes,
            SUCCESSES_REQUIRED,
        );
        if successes >= SUCCESSES_REQUIRED {
            tracing::info!(
                "[{}] OK: the higher priority track was received faster",
                scenario
            );
            return Ok(());
        }
        let remaining = ATTEMPTS_MAX - attempt;
        anyhow::ensure!(
            successes + remaining >= SUCCESSES_REQUIRED,
            "[{}] the higher priority track was not received faster \
             ({} of {} successes after {} attempts)",
            scenario,
            successes,
            SUCCESSES_REQUIRED,
            attempt,
        );
    }
    unreachable!("the vote always resolves within ATTEMPTS_MAX attempts");
}

async fn run_scenarios() -> anyhow::Result<()> {
    // ---- A: subscriber priority decides the transmission order ----
    run_scenario_with_vote(
        "subscriber",
        TrackSpec {
            track_name: "low",
            subscriber_priority: 200,
            publisher_priority: 128,
        },
        TrackSpec {
            track_name: "high",
            subscriber_priority: 10,
            publisher_priority: 128,
        },
    )
    .await?;

    // ---- B: equal subscriber priorities, publisher priority breaks the tie ----
    run_scenario_with_vote(
        "publisher",
        TrackSpec {
            track_name: "low",
            subscriber_priority: 128,
            publisher_priority: 200,
        },
        TrackSpec {
            track_name: "high",
            subscriber_priority: 128,
            publisher_priority: 10,
        },
    )
    .await?;

    tracing::info!("ALL OK: relay prioritized streams by subscriber and publisher priority");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    run_scenarios().await
}
