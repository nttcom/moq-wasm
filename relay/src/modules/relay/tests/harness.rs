//! Assembles the real data plane components (`relay.dataplane.*` spans) —
//! ingress `StreamReader`, `TrackCache`, notifier and `EgressRunner` — with
//! the mock clients from `mocks`, wired the same way production wires them.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot, watch};

use crate::modules::{
    relay::{
        cache::store::TrackCacheStore,
        egress::runner::EgressRunner,
        ingress::stream_reader::{StreamOpened, StreamReader},
        notifications::{track_event::TrackEvent, track_notifier::ObjectNotifyProducerMap},
    },
    types::TrackKey,
};

use super::mocks::{
    downstream_client::{
        EgressEvent, MockPublisher, header_group_on_stream, make_downstream_subscription,
        payloads_on_stream, stream_closed,
    },
    upstream_client::{UpstreamSubgroupStream, ordered_payload},
};

pub(crate) const OBJECT_COUNT: usize = 50;
const RECV_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct DataPlaneHarness {
    pub(crate) track_key: TrackKey,
    pub(crate) notify_map: Arc<ObjectNotifyProducerMap>,
    cache_store: Arc<TrackCacheStore>,
    opened_sender: mpsc::Sender<StreamOpened>,
    _stream_reader: StreamReader,
    _stop_sender: watch::Sender<bool>,
    stop_receiver: watch::Receiver<bool>,
}

pub(crate) struct RunningEgress {
    events: mpsc::UnboundedReceiver<EgressEvent>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for RunningEgress {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl DataPlaneHarness {
    pub(crate) fn new() -> Self {
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

    /// Opens one upstream subgroup stream in the ingress; the returned handle
    /// plays the publisher side of that stream.
    pub(crate) async fn open_upstream_stream(&self) -> UpstreamSubgroupStream {
        let (upstream_stream, receiver) = UpstreamSubgroupStream::open();
        self.opened_sender
            .send(StreamOpened {
                track_key: self.track_key.clone(),
                receiver,
                parent_span: tracing::Span::none(),
                stop_receiver: self.stop_receiver.clone(),
            })
            .await
            .expect("ingress stream reader should accept new streams");
        upstream_stream
    }

    /// Starts the egress runner the way `sequences::subscribe` does and waits
    /// for its readiness signal (production sends SUBSCRIBE_OK only after it).
    pub(crate) async fn start_egress(
        &self,
        largest_location: Option<moqt::Location>,
    ) -> RunningEgress {
        let (publisher, event_receiver) = MockPublisher::channel();
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
    pub(crate) async fn wait_end_of_group(
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
    pub(crate) async fn wait_largest_location(&self, expected: moqt::Location) -> moqt::Location {
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
/// Panics with the events collected so far when the data plane stalls, so a
/// lost-object bug reports as "stalled after N objects" rather than a hang.
pub(crate) async fn collect_until_closed(
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

pub(crate) fn assert_full_ordered_delivery(events: &[EgressEvent]) {
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
