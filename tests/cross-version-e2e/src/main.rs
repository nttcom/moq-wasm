//! E2E for a relay that accepts draft-14 and draft-18 peers on one port
//! (`RELAY_MOQT_VERSIONS=draft-14,draft-18`) and relays tracks between them.
//!
//!   1. A draft-14 publisher's track reaches a draft-18 subscriber, live and
//!      through a FETCH the relay answers from its cache; then the reverse.
//!   2. A draft-18 track that mixes subgroup streams and datagrams (§11)
//!      reaches a draft-14 subscriber in one forwarding preference, the one
//!      its first object came with: after a stream, datagram objects arrive
//!      on subgroup streams whose Subgroup ID is the Object ID; after a
//!      datagram, stream objects arrive as datagrams. A draft-18 subscriber
//!      gets the track mixed, as published.
//!   3. A draft-18 publisher answers SUBSCRIBE with a Mandatory Track
//!      Property (draft-18 §2.5.1). The relay understands none, so it
//!      refuses the track: a draft-18 subscriber gets UNSUPPORTED_EXTENSION
//!      and a draft-14 subscriber NOT_SUPPORTED, the closest draft-14 code.
//!   4. A draft-18 FETCH with FILL_TIMEOUT (draft-18 §10.2.5) of a range
//!      the relay caches only part of, from a publisher that answers an
//!      upstream FETCH but sends no object: with 0 the relay serves the
//!      cached part at once without fetching upstream, and with 300 ms it
//!      fetches upstream without forwarding the FILL_TIMEOUT and serves the
//!      same once the budget ran out. Either way the missing group is
//!      reported as an unknown range.
//!
//! Run `tests/cross-version-e2e/run.sh`, or start such a relay and
//! `MOQT_E2E_RELAY_URL=moqt://127.0.0.1:<port> cargo run -p cross-version-e2e`.

use std::{
    env,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use moqt::{
    ClientConfig, Endpoint, QUIC, RequestErrorCode, RequestFailure, Session, SessionEvent,
    Subscription, TrackObject, TrackProperties, TrackReader, Version,
    fetch::{Fetch, FetchOption, FetchParams},
    object::{
        ContentExists, ExtensionHeaders, ForwardingPreference, KeyValuePair, Location, Object,
        ObjectStatus, SubgroupStart, VariantType,
    },
    publish::PublishOption,
    subscribe::SubscribeOption,
};

const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const OBJECTS_PER_GROUP: u64 = 2;
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Lets the relay finish answering a SUBSCRIBE before the publisher sends.
const SETTLE: Duration = Duration::from_millis(300);

fn relay_url() -> String {
    env::var("MOQT_E2E_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string())
}

async fn connect(version: Version) -> anyhow::Result<Session> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        verify_certificate: false,
        follow_go_away: false,
        versions: vec![version],
        ..Default::default()
    })?;
    endpoint.connect(&relay_url()).await?.await
}

/// A namespace no earlier run used, so the relay cache holds nothing for it.
fn unique_namespace(scenario: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("anon/cross-version/{scenario}-{nanos}")
}

fn payload(group_id: u64, object_id: u64) -> String {
    format!("g{group_id}:o{object_id}")
}

fn object(group_id: u64, object_id: u64, forwarding: ForwardingPreference) -> Object {
    Object {
        location: Location {
            group_id,
            object_id,
        },
        forwarding,
        publisher_priority: PUBLISHER_PRIORITY,
        status: ObjectStatus::Normal,
        extension_headers: ExtensionHeaders::default(),
        payload: payload(group_id, object_id).into(),
        ends_group: false,
    }
}

struct Publication {
    session: Session,
    subscription: Subscription,
}

impl Publication {
    async fn start(version: Version, namespace: &str) -> anyhow::Result<Self> {
        let session = connect(version).await?;
        let subscription = session
            .publisher()
            .publish(
                namespace.to_string(),
                TRACK_NAME.to_string(),
                PublishOption::default(),
            )
            .await
            .context("PUBLISH was not accepted")?;
        Ok(Self {
            session,
            subscription,
        })
    }

    async fn send_stream_group(&self, group_id: u64) -> anyhow::Result<()> {
        let factory = self.session.publisher().create_stream(&self.subscription);
        let mut stream = factory
            .next()
            .await?
            .start(SubgroupStart {
                group_id,
                subgroup_id: 0,
                publisher_priority: PUBLISHER_PRIORITY,
                ends_group: false,
            })
            .await?;
        for object_id in 0..OBJECTS_PER_GROUP {
            stream
                .send_object(&object(
                    group_id,
                    object_id,
                    ForwardingPreference::Subgroup { subgroup_id: 0 },
                ))
                .await?;
        }
        stream.close().await
    }

    async fn send_datagram_group(&self, group_id: u64) -> anyhow::Result<()> {
        let mut sender = self.session.publisher().create_datagram(&self.subscription);
        for object_id in 0..OBJECTS_PER_GROUP {
            sender
                .send_object(&object(group_id, object_id, ForwardingPreference::Datagram))
                .await?;
        }
        Ok(())
    }
}

async fn subscribe(session: &Session, namespace: &str) -> anyhow::Result<TrackReader> {
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            namespace.to_string(),
            TRACK_NAME.to_string(),
            SubscribeOption::default(),
        )
        .await?;
    let receiver = subscriber.accept_data_receiver(&subscription).await?;
    Ok(TrackReader::new(receiver))
}

/// `(group, object, forwarding)` of the next `count` objects, sorted: the
/// subgroups and datagrams of a track are read concurrently.
async fn receive(
    reader: &mut TrackReader,
    count: u64,
) -> anyhow::Result<Vec<(u64, u64, ForwardingPreference)>> {
    let mut received = Vec::new();
    while received.len() < count as usize {
        let object: TrackObject = tokio::time::timeout(RECEIVE_TIMEOUT, reader.next_object())
            .await
            .context("the subscriber received no object in time")??
            .context("the track ended early")?;
        ensure!(
            object.payload == payload(object.group_id, object.object_id).as_bytes(),
            "unexpected payload at {{{}, {}}}",
            object.group_id,
            object.object_id
        );
        received.push((object.group_id, object.object_id, object.forwarding));
    }
    received.sort_by_key(|(group_id, object_id, _)| (*group_id, *object_id));
    Ok(received)
}

fn on_streams(groups: std::ops::Range<u64>) -> Vec<(u64, u64, ForwardingPreference)> {
    groups
        .flat_map(|group_id| {
            (0..OBJECTS_PER_GROUP).map(move |object_id| (group_id, object_id, subgroup(0)))
        })
        .collect()
}

fn subgroup(subgroup_id: u64) -> ForwardingPreference {
    ForwardingPreference::Subgroup { subgroup_id }
}

async fn fetch_ids(
    session: &Session,
    namespace: &str,
    end_group: u64,
) -> anyhow::Result<Vec<(u64, u64)>> {
    let mut subscriber = session.subscriber();
    let handle = subscriber
        .fetch(
            namespace.to_string(),
            TRACK_NAME.to_string(),
            Location {
                group_id: 0,
                object_id: 0,
            },
            // Object 0 of the End Location asks for the whole group.
            Location {
                group_id: end_group,
                object_id: 0,
            },
            FetchOption::default(),
        )
        .await?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    let mut ids = Vec::new();
    loop {
        match tokio::time::timeout(RECEIVE_TIMEOUT, receiver.receive())
            .await
            .context("the FETCH stalled")??
        {
            Fetch::Object(object) => {
                ids.push((object.location.group_id, object.location.object_id))
            }
            Fetch::End => return Ok(ids),
            Fetch::UnknownRange { last } => bail!("unknown range through {last:?}"),
        }
    }
}

async fn relays_live_and_fetched_objects(
    publisher_version: Version,
    subscriber_version: Version,
) -> anyhow::Result<()> {
    // Arrange
    let namespace = unique_namespace("live-and-fetch");
    let publication = Publication::start(publisher_version, &namespace).await?;
    let subscriber = connect(subscriber_version).await?;
    let mut reader = subscribe(&subscriber, &namespace).await?;
    tokio::time::sleep(SETTLE).await;

    // Act
    for group_id in 0..3 {
        publication.send_stream_group(group_id).await?;
    }
    let live = receive(&mut reader, 3 * OBJECTS_PER_GROUP).await?;
    let fetched = fetch_ids(&subscriber, &namespace, 1).await?;

    // Assert
    ensure!(live == on_streams(0..3), "live objects: {live:?}");
    let expected: Vec<(u64, u64)> = on_streams(0..2)
        .into_iter()
        .map(|(group_id, object_id, _)| (group_id, object_id))
        .collect();
    ensure!(fetched == expected, "fetched objects: {fetched:?}");
    tracing::info!(
        ?publisher_version,
        ?subscriber_version,
        "OK: live and fetched objects relayed"
    );
    Ok(())
}

async fn relays_a_mixed_track_in_one_preference_to_draft14(
    datagram_first: bool,
) -> anyhow::Result<()> {
    // Arrange
    let namespace = unique_namespace(if datagram_first {
        "mixed-datagram-first"
    } else {
        "mixed-stream-first"
    });
    let publication = Publication::start(Version::Draft18, &namespace).await?;
    let draft14 = connect(Version::Draft14).await?;
    let draft18 = connect(Version::Draft18).await?;
    let mut draft14_reader = subscribe(&draft14, &namespace).await?;
    let mut draft18_reader = subscribe(&draft18, &namespace).await?;
    tokio::time::sleep(SETTLE).await;
    let send_group = |datagram: bool, group_id: u64| {
        let publication = &publication;
        async move {
            if datagram {
                publication.send_datagram_group(group_id).await
            } else {
                publication.send_stream_group(group_id).await
            }
        }
    };

    // Act: the first group reaches both subscribers before the other kind
    send_group(datagram_first, 0).await?;
    let mut draft14_received = receive(&mut draft14_reader, OBJECTS_PER_GROUP).await?;
    let mut draft18_received = receive(&mut draft18_reader, OBJECTS_PER_GROUP).await?;
    send_group(!datagram_first, 1).await?;
    send_group(datagram_first, 2).await?;
    draft14_received.extend(receive(&mut draft14_reader, 2 * OBJECTS_PER_GROUP).await?);
    draft18_received.extend(receive(&mut draft18_reader, 2 * OBJECTS_PER_GROUP).await?);

    // Assert
    let as_published = |group_id: u64, _object_id: u64| {
        let datagram = (group_id == 1) != datagram_first;
        if datagram {
            ForwardingPreference::Datagram
        } else {
            subgroup(0)
        }
    };
    let as_first =
        |group_id: u64, object_id: u64| match (datagram_first, as_published(group_id, object_id)) {
            (true, _) => ForwardingPreference::Datagram,
            // draft-14 §10.4.4 puts a datagram object in the subgroup of its Object ID.
            (false, ForwardingPreference::Datagram) => subgroup(object_id),
            (false, forwarding) => forwarding,
        };
    let expected = |forwarding: &dyn Fn(u64, u64) -> ForwardingPreference| {
        (0..3)
            .flat_map(|group_id| (0..OBJECTS_PER_GROUP).map(move |object_id| (group_id, object_id)))
            .map(|(group_id, object_id)| (group_id, object_id, forwarding(group_id, object_id)))
            .collect::<Vec<_>>()
    };
    ensure!(
        draft14_received == expected(&as_first),
        "draft-14 subscriber: {draft14_received:?}"
    );
    ensure!(
        draft18_received == expected(&as_published),
        "draft-18 subscriber: {draft18_received:?}"
    );
    tracing::info!(
        datagram_first,
        "OK: mixed track kept to one preference for draft-14"
    );
    Ok(())
}

fn mandatory_track_properties() -> TrackProperties {
    TrackProperties::from_pairs(vec![KeyValuePair {
        key: 0x4000,
        value: VariantType::Even(1),
    }])
}

async fn refuses_a_track_with_a_mandatory_track_property() -> anyhow::Result<()> {
    // Arrange: the publisher answers every SUBSCRIBE with the property
    let namespace = unique_namespace("mandatory");
    let publisher = connect(Version::Draft18).await?;
    publisher
        .publisher()
        .publish_namespace(namespace.clone())
        .await
        .context("PUBLISH_NAMESPACE was not accepted")?;
    let answer = async {
        loop {
            match publisher.receive_event().await? {
                SessionEvent::Subscribe(mut handler) => {
                    handler.track_properties = mandatory_track_properties();
                    handler.ok(0, ContentExists::False).await?;
                }
                SessionEvent::Disconnected() => bail!("the publisher was disconnected"),
                _ => {}
            }
        }
    };
    let subscribe_with = |version: Version| {
        let namespace = namespace.clone();
        async move {
            let session = connect(version).await?;
            anyhow::Ok(subscribe(&session, &namespace).await.err())
        }
    };

    // Act
    let refusals = tokio::select! {
        result = answer => {
            result?;
            unreachable!("the answer loop only ends with an error")
        }
        refusals = async {
            anyhow::Ok((
                subscribe_with(Version::Draft18).await?,
                subscribe_with(Version::Draft14).await?,
            ))
        } => refusals?,
    };

    // Assert
    let refused_with = |refusal: &Option<anyhow::Error>| match refusal
        .as_ref()?
        .downcast_ref::<RequestFailure>()?
    {
        RequestFailure::Rejected { code, .. } => Some(*code),
        _ => None,
    };
    ensure!(
        refused_with(&refusals.0) == Some(RequestErrorCode::UnsupportedExtension),
        "draft-18 subscriber: {:?}",
        refusals.0
    );
    ensure!(
        refused_with(&refusals.1) == Some(RequestErrorCode::NotSupported),
        "draft-14 subscriber: {:?}",
        refusals.1
    );
    tracing::info!("OK: a track with a Mandatory Track Property is refused");
    Ok(())
}

/// What a draft-18 FETCH of groups 0 and 1 with `fill_timeout` returned,
/// with the relay caching group 1 only.
struct FilledFetch {
    /// `g/o` per object, `unknown..g/o` per unknown range, then `end`.
    steps: Vec<String>,
    took: Duration,
    /// The FILL_TIMEOUT of each FETCH the relay sent the publisher.
    upstream_fill_timeouts: Vec<Option<Duration>>,
}

async fn fetch_with_fill_timeout(fill_timeout: Duration) -> anyhow::Result<FilledFetch> {
    let namespace = unique_namespace("fill-timeout");
    let publication = Publication::start(Version::Draft18, &namespace).await?;
    let session = connect(Version::Draft18).await?;
    let mut reader = subscribe(&session, &namespace).await?;
    tokio::time::sleep(SETTLE).await;
    publication.send_stream_group(1).await?;
    receive(&mut reader, OBJECTS_PER_GROUP).await?;
    let upstream_fill_timeouts = Mutex::new(Vec::new());
    // The publisher answers FETCH_OK and never opens the response stream.
    let answer = async {
        let mut answered = Vec::new();
        loop {
            match publication.session.receive_event().await? {
                SessionEvent::Fetch(handler) => {
                    upstream_fill_timeouts
                        .lock()
                        .unwrap()
                        .push(handler.fill_timeout());
                    let FetchParams::Standalone { end_location, .. } =
                        handler.fetch.fetch_params.clone()
                    else {
                        bail!("the relay sent a Joining FETCH upstream");
                    };
                    handler.ok(false, end_location).await?;
                    answered.push(handler);
                }
                SessionEvent::Disconnected() => bail!("the publisher was disconnected"),
                _ => {}
            }
        }
    };
    let started = Instant::now();
    let fetched = async {
        let mut subscriber = session.subscriber();
        let handle = subscriber
            .fetch(
                namespace.clone(),
                TRACK_NAME.to_string(),
                Location {
                    group_id: 0,
                    object_id: 0,
                },
                Location {
                    group_id: 1,
                    object_id: 0,
                },
                FetchOption {
                    fill_timeout: Some(fill_timeout),
                    ..FetchOption::default()
                },
            )
            .await?;
        let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
        let mut steps = Vec::new();
        loop {
            match tokio::time::timeout(RECEIVE_TIMEOUT, receiver.receive())
                .await
                .context("the FETCH stalled")??
            {
                Fetch::Object(object) => steps.push(format!(
                    "{}/{}",
                    object.location.group_id, object.location.object_id
                )),
                Fetch::UnknownRange { last } => {
                    steps.push(format!("unknown..{}/{}", last.group_id, last.object_id))
                }
                Fetch::End => {
                    steps.push("end".to_string());
                    return anyhow::Ok(steps);
                }
            }
        }
    };

    let steps = tokio::select! {
        result = answer => {
            result?;
            unreachable!("the answer loop only ends with an error")
        }
        steps = fetched => steps?,
    };
    Ok(FilledFetch {
        steps,
        took: started.elapsed(),
        upstream_fill_timeouts: upstream_fill_timeouts.into_inner().unwrap(),
    })
}

/// Group 0 unknown, then the cached group 1.
fn group_0_unknown_then_group_1() -> Vec<String> {
    vec![
        format!("unknown..0/{}", u64::MAX),
        "1/0".to_string(),
        "1/1".to_string(),
        "end".to_string(),
    ]
}

async fn a_zero_fill_timeout_serves_the_cache_without_waiting() -> anyhow::Result<()> {
    // Act
    let fetched = fetch_with_fill_timeout(Duration::ZERO).await?;

    // Assert: draft-18 §10.2.5 MUST NOT wait for upstream delivery
    ensure!(
        fetched.steps == group_0_unknown_then_group_1(),
        "{:?}",
        fetched.steps
    );
    ensure!(
        fetched.upstream_fill_timeouts.is_empty(),
        "the relay fetched upstream: {:?}",
        fetched.upstream_fill_timeouts
    );
    tracing::info!(took = ?fetched.took, "OK: FILL_TIMEOUT 0 served from the cache");
    Ok(())
}

async fn a_fill_timeout_bounds_the_upstream_wait() -> anyhow::Result<()> {
    // Act
    let budget = Duration::from_millis(300);
    let fetched = fetch_with_fill_timeout(budget).await?;

    // Assert: draft-18 §10.2.5, the rest is unknown once the budget ran out
    ensure!(
        fetched.steps == group_0_unknown_then_group_1(),
        "{:?}",
        fetched.steps
    );
    ensure!(
        fetched.upstream_fill_timeouts == vec![None],
        "upstream FETCHes: {:?}",
        fetched.upstream_fill_timeouts
    );
    ensure!(fetched.took >= budget, "served after {:?}", fetched.took);
    tracing::info!(took = ?fetched.took, "OK: FILL_TIMEOUT bounded the upstream wait");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    relays_live_and_fetched_objects(Version::Draft14, Version::Draft18).await?;
    relays_live_and_fetched_objects(Version::Draft18, Version::Draft14).await?;
    relays_a_mixed_track_in_one_preference_to_draft14(false).await?;
    relays_a_mixed_track_in_one_preference_to_draft14(true).await?;
    refuses_a_track_with_a_mandatory_track_property().await?;
    a_zero_fill_timeout_serves_the_cache_without_waiting().await?;
    a_fill_timeout_bounds_the_upstream_wait().await?;
    tracing::info!("cross-version e2e passed");
    Ok(())
}
