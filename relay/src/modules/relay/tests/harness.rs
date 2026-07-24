//! Test harness for the live forwarding pipeline: wires the real ingress
//! `StreamReader`, `TrackCache`, notifier and `EgressRunner` together with no
//! QUIC involved. Only the two boundaries are substituted: the upstream
//! `StreamReceiver` is fed through a channel so tests control exactly when
//! each object (and the FIN) becomes visible to the ingress, and the
//! downstream `Publisher` captures every egressed object, tagged with the
//! downstream uni-stream it was sent on.

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

pub(super) const OBJECT_COUNT: usize = 50;
const RECV_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Upstream side
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

/// Plays the publisher side of one upstream subgroup stream.
pub(super) struct UpstreamFeed {
    sender: mpsc::UnboundedSender<FeedItem>,
}

impl UpstreamFeed {
    pub(super) fn header(&self, group_id: u64) {
        self.sender
            .send(Ok(Some(make_header(group_id))))
            .expect("ingress feed should be open");
    }

    pub(super) fn object(&self, index: usize) {
        let delta = if index == 0 { 0 } else { 1 };
        self.sender
            .send(Ok(Some(make_payload_object(delta, ordered_payload(index)))))
            .expect("ingress feed should be open");
    }

    pub(super) fn fin(&self) {
        self.sender
            .send(Ok(None))
            .expect("ingress feed should be open");
    }
}

// ---------------------------------------------------------------------------
// Downstream side
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(super) enum EgressEvent {
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

pub(super) fn ordered_payload(index: usize) -> Bytes {
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
// Pipeline wiring
// ---------------------------------------------------------------------------

pub(super) struct PipelineHarness {
    pub(super) track_key: TrackKey,
    pub(super) notify_map: Arc<ObjectNotifyProducerMap>,
    cache_store: Arc<TrackCacheStore>,
    opened_sender: mpsc::Sender<StreamOpened>,
    _stream_reader: StreamReader,
    _stop_sender: watch::Sender<bool>,
    stop_receiver: watch::Receiver<bool>,
}

pub(super) struct RunningEgress {
    events: mpsc::UnboundedReceiver<EgressEvent>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for RunningEgress {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl PipelineHarness {
    pub(super) fn new() -> Self {
        let track_key = TrackKey::new("ns", "track");
        let cache_store = Arc::new(TrackCacheStore::new());
        let notify_map = Arc::new(ObjectNotifyProducerMap::new());
        let (opened_sender, opened_receiver) = mpsc::channel(16);
        let stream_reader =
            StreamReader::run(opened_receiver, cache_store.clone(), notify_map.clone());
        let (stop_sender, stop_receiver) = watch::channel(false);
        Self {
            track_key,
            notify_map,
            cache_store,
            opened_sender,
            _stream_reader: stream_reader,
            _stop_sender: stop_sender,
            stop_receiver,
        }
    }

    /// Opens one upstream subgroup stream in the ingress; the returned feed
    /// plays the publisher side of that stream.
    pub(super) async fn open_upstream_stream(&self) -> UpstreamFeed {
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
    pub(super) async fn start_egress(
        &self,
        largest_location: Option<moqt::Location>,
    ) -> RunningEgress {
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
    pub(super) async fn wait_end_of_group(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<TrackEvent>,
    ) {
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
    pub(super) async fn wait_largest_location(&self, expected: moqt::Location) -> moqt::Location {
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

// ---------------------------------------------------------------------------
// Collection and assertion helpers
// ---------------------------------------------------------------------------

/// Receives egress events until `closed_streams` downstream streams closed.
/// Panics with the events collected so far when the pipeline stalls, so a
/// lost-object bug reports as "stalled after N objects" rather than a hang.
pub(super) async fn collect_until_closed(
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
pub(super) fn payloads_on_stream(events: &[EgressEvent], stream: usize) -> Vec<Bytes> {
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

pub(super) fn stream_closed(events: &[EgressEvent], stream: usize) -> bool {
    events
        .iter()
        .any(|event| matches!(event, EgressEvent::Closed { stream: s } if *s == stream))
}

pub(super) fn header_group_on_stream(events: &[EgressEvent], stream: usize) -> Option<u64> {
    events.iter().find_map(|event| match event {
        EgressEvent::Object {
            stream: s,
            object: DataObject::SubgroupHeader(header),
        } if *s == stream => Some(header.group_id),
        _ => None,
    })
}

pub(super) fn assert_full_ordered_delivery(events: &[EgressEvent]) {
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
