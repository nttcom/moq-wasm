//! In-process integration tests for the live forwarding pipeline:
//! ingress `StreamReader` -> `TrackCache` -> `EgressRunner` (scheduler +
//! group sender) -> downstream `DataSender`, with no QUIC involved.
//!
//! These target the cascading-relay E2E flake "stream ended before receiving
//! 50 objects": a publisher that answers SUBSCRIBE with a burst of objects on
//! one subgroup stream and closes it immediately (FIN) races the relay's
//! subscribe handling, and the downstream subscriber observes a truncated
//! stream. The tests pin the pipeline behavior at each stage of that race so
//! a failure names the stage instead of an opaque E2E timeout.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot, watch};

use crate::modules::{
    core::{
        data_object::DataObject,
        data_receiver::stream_receiver::StreamReceiver,
        data_sender::{
            DataSender, fetch_sender::FetchSender, stream_sender_factory::StreamSenderFactory,
        },
        publisher::Publisher,
        subscription::DownstreamSubscription,
    },
    relay::{
        cache::store::TrackCacheStore,
        egress::runner::EgressRunner,
        ingress::stream_reader::{StreamOpened, StreamReader},
        notifications::{track_event::TrackEvent, track_notifier::ObjectNotifyProducerMap},
    },
    types::TrackKey,
};

const OBJECT_COUNT: usize = 50;
const RECV_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Upstream side: a StreamReceiver fed through a channel, so tests control
// exactly when each object (and the FIN) becomes visible to the ingress.
// ---------------------------------------------------------------------------

type FeedItem = Result<Option<DataObject>, moqt::StreamReceiveError>;

struct ChannelStreamReceiver {
    receiver: mpsc::UnboundedReceiver<FeedItem>,
}

#[async_trait::async_trait]
impl StreamReceiver for ChannelStreamReceiver {
    async fn receive_object(&mut self) -> Result<Option<DataObject>, moqt::StreamReceiveError> {
        match self.receiver.recv().await {
            Some(item) => item,
            // Dropped feed sender behaves like a FIN.
            None => Ok(None),
        }
    }
}

struct UpstreamFeed {
    sender: mpsc::UnboundedSender<FeedItem>,
}

impl UpstreamFeed {
    fn header(&self, group_id: u64) {
        self.sender
            .send(Ok(Some(make_header(group_id))))
            .expect("ingress feed should be open");
    }

    fn object(&self, index: usize) {
        let delta = if index == 0 { 0 } else { 1 };
        self.sender
            .send(Ok(Some(make_payload_object(delta, ordered_payload(index)))))
            .expect("ingress feed should be open");
    }

    fn fin(&self) {
        self.sender
            .send(Ok(None))
            .expect("ingress feed should be open");
    }
}

// ---------------------------------------------------------------------------
// Downstream side: a Publisher whose stream senders capture every egressed
// object, tagged with the downstream uni-stream they were sent on.
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum EgressEvent {
    Object { stream: usize, object: DataObject },
    Closed { stream: usize },
}

struct CapturingDataSender {
    stream: usize,
    events: mpsc::UnboundedSender<EgressEvent>,
}

#[async_trait::async_trait]
impl DataSender for CapturingDataSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        self.events
            .send(EgressEvent::Object {
                stream: self.stream,
                object,
            })
            .map_err(|_| anyhow::anyhow!("egress capture receiver dropped"))
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        self.events
            .send(EgressEvent::Closed {
                stream: self.stream,
            })
            .map_err(|_| anyhow::anyhow!("egress capture receiver dropped"))
    }
}

struct CapturingStreamSenderFactory {
    next_stream: Arc<AtomicUsize>,
    events: mpsc::UnboundedSender<EgressEvent>,
}

#[async_trait::async_trait]
impl StreamSenderFactory for CapturingStreamSenderFactory {
    async fn next(&mut self) -> anyhow::Result<Box<dyn DataSender>> {
        let stream = self.next_stream.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CapturingDataSender {
            stream,
            events: self.events.clone(),
        }))
    }
}

struct CapturingPublisher {
    next_stream: Arc<AtomicUsize>,
    events: mpsc::UnboundedSender<EgressEvent>,
}

#[async_trait::async_trait]
impl Publisher for CapturingPublisher {
    async fn send_publish_namespace(&self, _namespaces: String) -> anyhow::Result<()> {
        unreachable!("not used by the egress pipeline under test")
    }

    async fn send_publish_namespace_done(&self, _namespace: String) -> anyhow::Result<()> {
        unreachable!("not used by the egress pipeline under test")
    }

    async fn send_publish(
        &self,
        _track_namespace: String,
        _track_name: String,
    ) -> anyhow::Result<DownstreamSubscription> {
        unreachable!("not used by the egress pipeline under test")
    }

    fn new_stream_factory(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        Box::new(CapturingStreamSenderFactory {
            next_stream: self.next_stream.clone(),
            events: self.events.clone(),
        })
    }

    fn new_datagram(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DataSender> {
        unreachable!("not used by the egress pipeline under test")
    }

    async fn new_fetch_sender(&self, _request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        unreachable!("not used by the egress pipeline under test")
    }
}

// ---------------------------------------------------------------------------
// Object builders (payloads carry their index so ordering is verifiable).
// ---------------------------------------------------------------------------

fn ordered_payload(index: usize) -> Bytes {
    Bytes::from(format!("ordered-object-{index}"))
}

fn make_header(group_id: u64) -> DataObject {
    DataObject::SubgroupHeader(moqt::SubgroupHeader::new(
        0,
        group_id,
        moqt::SubgroupId::Value(0),
        128,
        false,
        false,
    ))
}

fn make_payload_object(object_id_delta: u64, payload: Bytes) -> DataObject {
    let message_type =
        moqt::SubgroupHeader::new(0, 0, moqt::SubgroupId::Value(0), 128, false, false).message_type;
    DataObject::SubgroupObject(moqt::SubgroupObjectField {
        message_type,
        object_id_delta,
        extension_headers: moqt::ExtensionHeaders::default(),
        subgroup_object: moqt::SubgroupObject::new_payload(payload),
    })
}

/// A LargestObject/Ascending downstream subscription, matching what the
/// cascading-relay E2E subscriber sends.
fn make_downstream_subscription() -> DownstreamSubscription {
    DownstreamSubscription::from(moqt::Subscription::SubscriberInitiated(
        moqt::SubscriberInitiatedSubscription {
            request_id: 0,
            track_namespace: "ns".to_string(),
            track_name: "track".to_string(),
            track_alias: 0,
            expires: 0,
            group_order: moqt::GroupOrder::Ascending,
            content_exists: moqt::ContentExists::False,
            filter_type: moqt::FilterType::LargestObject,
            delivery_timeout: None,
        },
    ))
}

// ---------------------------------------------------------------------------
// Harness wiring the real ingress reader, cache, notifier and egress runner.
// ---------------------------------------------------------------------------

struct PipelineHarness {
    track_key: TrackKey,
    cache_store: Arc<TrackCacheStore>,
    notify_map: Arc<ObjectNotifyProducerMap>,
    opened_sender: mpsc::Sender<StreamOpened>,
    _stream_reader: StreamReader,
    _stop_sender: watch::Sender<bool>,
    stop_receiver: watch::Receiver<bool>,
}

struct RunningEgress {
    events: mpsc::UnboundedReceiver<EgressEvent>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for RunningEgress {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl PipelineHarness {
    fn new() -> Self {
        let track_key = TrackKey::new("ns", "track");
        let cache_store = Arc::new(TrackCacheStore::new());
        let notify_map = Arc::new(ObjectNotifyProducerMap::new());
        let (opened_sender, opened_receiver) = mpsc::channel(16);
        let stream_reader =
            StreamReader::run(opened_receiver, cache_store.clone(), notify_map.clone());
        let (stop_sender, stop_receiver) = watch::channel(false);
        Self {
            track_key,
            cache_store,
            notify_map,
            opened_sender,
            _stream_reader: stream_reader,
            _stop_sender: stop_sender,
            stop_receiver,
        }
    }

    /// Opens one upstream subgroup stream in the ingress; the returned feed
    /// plays the publisher side of that stream.
    async fn open_upstream_stream(&self) -> UpstreamFeed {
        let (feed_sender, feed_receiver) = mpsc::unbounded_channel();
        self.opened_sender
            .send(StreamOpened {
                track_key: self.track_key.clone(),
                receiver: Box::new(ChannelStreamReceiver {
                    receiver: feed_receiver,
                }),
                parent_span: tracing::Span::none(),
                stop_receiver: self.stop_receiver.clone(),
            })
            .await
            .expect("ingress stream reader should accept new streams");
        UpstreamFeed {
            sender: feed_sender,
        }
    }

    /// Starts the egress runner the way `sequences::subscribe` does and waits
    /// for its readiness signal (production sends SUBSCRIBE_OK only after it).
    async fn start_egress(&self, largest_location: Option<moqt::Location>) -> RunningEgress {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let publisher = CapturingPublisher {
            next_stream: Arc::new(AtomicUsize::new(0)),
            events: event_sender,
        };
        let (ready_sender, ready_receiver) = oneshot::channel();
        let runner = EgressRunner::new(
            self.track_key.clone(),
            self.cache_store.get_or_create(&self.track_key),
            self.notify_map.get_or_create(&self.track_key),
            Box::new(publisher),
            make_downstream_subscription(),
            ready_sender,
            largest_location,
        );
        let handle = tokio::spawn(async move {
            let _ = runner.run().await;
        });
        tokio::time::timeout(RECV_TIMEOUT, ready_receiver)
            .await
            .expect("egress runner should signal readiness")
            .expect("egress readiness should not be dropped")
            .expect("egress runner should start");
        RunningEgress {
            events: event_receiver,
            handle,
        }
    }

    /// Waits until the ingress has fully ingested a subgroup (its EndOfGroup
    /// notification fired). Subscribe to events before feeding the FIN.
    async fn wait_end_of_group(&self, events: &mut tokio::sync::broadcast::Receiver<TrackEvent>) {
        loop {
            let event = tokio::time::timeout(RECV_TIMEOUT, events.recv())
                .await
                .expect("ingress should finish ingesting the subgroup")
                .expect("track event channel should stay open");
            if matches!(event, TrackEvent::EndOfGroup) {
                return;
            }
        }
    }

    /// Polls the cache until its Largest Location reaches `expected`,
    /// mirroring the moment `sequences::subscribe` resolves it.
    async fn wait_largest_location(&self, expected: moqt::Location) -> moqt::Location {
        let cache = self.cache_store.get_or_create(&self.track_key);
        let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
        loop {
            if let Some(largest) = cache.largest_location().await
                && (largest.group_id, largest.object_id) >= (expected.group_id, expected.object_id)
            {
                return largest;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "cache never reached largest location {{{}, {}}}",
                expected.group_id,
                expected.object_id
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

/// Receives egress events until `closed_streams` downstream streams closed.
/// Panics with the events collected so far when the pipeline stalls, so a
/// lost-object bug reports as "stalled after N objects" rather than a hang.
async fn collect_until_closed(
    egress: &mut RunningEgress,
    closed_streams: usize,
) -> Vec<EgressEvent> {
    let mut events = Vec::new();
    let mut closed = 0;
    while closed < closed_streams {
        let event = match tokio::time::timeout(RECV_TIMEOUT, egress.events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => panic!("egress capture channel closed early; events so far: {events:?}"),
            Err(_) => panic!(
                "egress stalled before closing {closed_streams} stream(s); events so far: {events:?}"
            ),
        };
        if matches!(event, EgressEvent::Closed { .. }) {
            closed += 1;
        }
        events.push(event);
    }
    events
}

/// Payloads sent on one downstream stream, in send order.
fn payloads_on_stream(events: &[EgressEvent], stream: usize) -> Vec<Bytes> {
    events
        .iter()
        .filter_map(|event| match event {
            EgressEvent::Object {
                stream: s,
                object: DataObject::SubgroupObject(field),
            } if *s == stream => match &field.subgroup_object {
                moqt::SubgroupObject::Payload { data, .. } => Some(data.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn stream_closed(events: &[EgressEvent], stream: usize) -> bool {
    events
        .iter()
        .any(|event| matches!(event, EgressEvent::Closed { stream: s } if *s == stream))
}

fn header_group_on_stream(events: &[EgressEvent], stream: usize) -> Option<u64> {
    events.iter().find_map(|event| match event {
        EgressEvent::Object {
            stream: s,
            object: DataObject::SubgroupHeader(header),
        } if *s == stream => Some(header.group_id),
        _ => None,
    })
}

fn assert_full_ordered_delivery(events: &[EgressEvent]) {
    assert_eq!(
        header_group_on_stream(events, 0),
        Some(0),
        "downstream stream should start with the group 0 subgroup header"
    );
    let payloads = payloads_on_stream(events, 0);
    let expected: Vec<Bytes> = (0..OBJECT_COUNT).map(ordered_payload).collect();
    assert_eq!(
        payloads.len(),
        expected.len(),
        "downstream stream ended before receiving {OBJECT_COUNT} objects (got {})",
        payloads.len()
    );
    assert_eq!(payloads, expected, "objects must arrive in publish order");
    assert!(
        stream_closed(events, 0),
        "the downstream stream should be closed after the last object"
    );
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// Baseline: egress is running (largest resolved as None before any object
/// existed) when the publisher bursts one subgroup of 50 objects and FINs
/// immediately. Every object must reach the downstream before its stream
/// closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_publish_with_immediate_fin_delivers_all_objects() {
    for round in 0..100 {
        let harness = PipelineHarness::new();
        let mut egress = harness.start_egress(None).await;

        let feed = harness.open_upstream_stream().await;
        feed.header(0);
        for index in 0..OBJECT_COUNT {
            feed.object(index);
        }
        feed.fin();

        let events = collect_until_closed(&mut egress, 1).await;
        let payloads = payloads_on_stream(&events, 0);
        assert_eq!(
            payloads.len(),
            OBJECT_COUNT,
            "round {round}: downstream stream ended before receiving {OBJECT_COUNT} objects"
        );
        assert_full_ordered_delivery(&events);
    }
}

/// The egress starts while the burst is already in flight (subscribe-time
/// largest was resolved as None before the burst reached the relay). The
/// scheduler must catch up from the cache regardless of whether the
/// StreamOpened notification fired before it subscribed to track events.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn egress_start_racing_ingest_burst_delivers_all_objects() {
    for round in 0..100 {
        let harness = PipelineHarness::new();

        let feed = harness.open_upstream_stream().await;
        feed.header(0);
        for index in 0..OBJECT_COUNT {
            feed.object(index);
        }
        feed.fin();

        // No await between feeding and starting the egress: the ingest and
        // the egress startup interleave differently on every round.
        let mut egress = harness.start_egress(None).await;

        let events = collect_until_closed(&mut egress, 1).await;
        let payloads = payloads_on_stream(&events, 0);
        assert_eq!(
            payloads.len(),
            OBJECT_COUNT,
            "round {round}: downstream stream ended before receiving {OBJECT_COUNT} objects"
        );
        assert_full_ordered_delivery(&events);
    }
}

/// The subscriber arrives after the whole subgroup was ingested and closed
/// (fastest possible FIN). A closed subgroup must still be drained in full;
/// closing must never truncate objects already in the cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_subgroup_is_drained_in_full() {
    let harness = PipelineHarness::new();
    let mut track_events = harness
        .notify_map
        .get_or_create(&harness.track_key)
        .subscribe();

    let feed = harness.open_upstream_stream().await;
    feed.header(0);
    for index in 0..OBJECT_COUNT {
        feed.object(index);
    }
    feed.fin();
    harness.wait_end_of_group(&mut track_events).await;

    let mut egress = harness.start_egress(None).await;

    let events = collect_until_closed(&mut egress, 1).await;
    assert_full_ordered_delivery(&events);
}

/// Reproduces the cascading-relay E2E flake "stream ended before receiving 50
/// objects" (ordered objects scenario, e.g. CI runs 29805931921, 29481445955,
/// 29477073181).
///
/// Production ordering in `sequences::subscribe`: the upstream ingress starts
/// on SUBSCRIBE_OK, and only afterwards `accept_downstream_subscription`
/// resolves the subscribe-time Largest Location from the live cache
/// (`resolve_subscribe_largest`). A publisher that answers SUBSCRIBE by
/// bursting objects immediately (the E2E publisher sends all 50 within ~1ms)
/// lands part of the burst in the cache inside that window. The resolved
/// largest then points into the burst, the LargestObject filter starts
/// delivery just after it, and the downstream subscriber - whose SUBSCRIBE is
/// what triggered this publish, and which observed no content at subscribe
/// time - permanently loses the head of the group.
///
/// The relay must not let objects that arrived after the triggering SUBSCRIBE
/// shift that subscriber's delivery start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "known bug: in-flight burst shifts the resolved largest location and the head of the group is lost; un-ignore with the fix"]
async fn largest_location_resolved_mid_burst_must_not_skip_head_objects() {
    const IN_FLIGHT_BEFORE_RESOLVE: usize = 10;

    let harness = PipelineHarness::new();

    // The burst races ahead: the first objects reach the cache before the
    // subscribe sequence resolves the largest location.
    let feed = harness.open_upstream_stream().await;
    feed.header(0);
    for index in 0..IN_FLIGHT_BEFORE_RESOLVE {
        feed.object(index);
    }
    let largest = harness
        .wait_largest_location(moqt::Location {
            group_id: 0,
            object_id: (IN_FLIGHT_BEFORE_RESOLVE - 1) as u64,
        })
        .await;

    // What `accept_downstream_subscription` does now: largest from the live
    // cache -> EgressStartRequest -> SUBSCRIBE_OK.
    let mut egress = harness.start_egress(Some(largest)).await;

    for index in IN_FLIGHT_BEFORE_RESOLVE..OBJECT_COUNT {
        feed.object(index);
    }
    feed.fin();

    let events = collect_until_closed(&mut egress, 1).await;
    assert_full_ordered_delivery(&events);
}

/// Multi-group variant closer to the media E2E shape: several subgroup
/// streams in sequence, each closed with FIN right after its last object.
/// Every group must arrive complete on its own downstream stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sequential_groups_with_fast_fin_each_arrive_complete() {
    const GROUPS: u64 = 10;
    const OBJECTS_PER_GROUP: usize = 5;

    let harness = PipelineHarness::new();
    let mut egress = harness.start_egress(None).await;

    for group_id in 0..GROUPS {
        let feed = harness.open_upstream_stream().await;
        feed.header(group_id);
        for index in 0..OBJECTS_PER_GROUP {
            feed.object(index);
        }
        feed.fin();
    }

    let events = collect_until_closed(&mut egress, GROUPS as usize).await;

    let mut delivered_groups = Vec::new();
    for stream in 0..GROUPS as usize {
        let group_id = header_group_on_stream(&events, stream)
            .unwrap_or_else(|| panic!("stream {stream} should carry a subgroup header"));
        let payloads = payloads_on_stream(&events, stream);
        let expected: Vec<Bytes> = (0..OBJECTS_PER_GROUP).map(ordered_payload).collect();
        assert_eq!(
            payloads, expected,
            "group {group_id} on stream {stream} must arrive complete and in order"
        );
        assert!(
            stream_closed(&events, stream),
            "stream {stream} should be closed after its last object"
        );
        delivered_groups.push(group_id);
    }
    delivered_groups.sort_unstable();
    assert_eq!(
        delivered_groups,
        (0..GROUPS).collect::<Vec<_>>(),
        "every group must be delivered exactly once"
    );
}
