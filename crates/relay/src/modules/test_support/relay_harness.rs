use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::modules::{
    auth::verified_token::VerifiedToken,
    data_plane::{
        cache::{
            cached_object::CacheStamp,
            subgroup_key::SubgroupKey,
            track_cache::{NextObject, SubgroupRun, TrackCache},
        },
        egress::{
            coordinator::EgressFetchRequest, fetch_delivery::deliver_fetch, runner::EgressRunner,
        },
        ingress::{
            datagram_reader::read_datagrams,
            stream_reader::{accept_streams, read_stream},
            track_ingest_task::TrackIngest,
        },
    },
    domain::{
        delivery_control::DeliveryControl,
        delivery_stats::{DeliveryCounters, DeliveryStats},
        pub_sub_directory::{DownstreamRunnerSignals, entry::PublishDoneReason},
        session_id::SessionId,
        track_key::TrackKey,
    },
    session::{
        data_object::DataObject, session_event::SessionEvent, session_repository::SessionRepository,
    },
    test_support::mock_session::session_repository_with_session,
};

pub(crate) mod fixtures;
mod mocks;

pub(crate) use self::mocks::downstream_client::{FetchSent, MockPublisherObservers, Sent};
pub(crate) use self::mocks::upstream_client::{FailingUpstream, UpstreamSubgroupStream};

pub(crate) use self::fixtures::data_object::ordered_payload;

use self::{
    fixtures::{location, subscription::make_subscription},
    mocks::downstream_client::{
        MockDownstreamSession, MockFetchSender, MockPublisher, SentPublishDone,
    },
};

pub(crate) const OBJECT_COUNT: usize = 50;
pub(crate) const PUBLISHER_SESSION_ID: SessionId = 1;
const RECV_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct RelayHarness {
    ingest: TrackIngest,
    session_event_receiver: mpsc::UnboundedReceiver<SessionEvent>,
    stop_sender: watch::Sender<bool>,
}

pub(crate) struct EgressRunnerHandle {
    sent: mpsc::UnboundedReceiver<Sent>,
    priorities: mpsc::UnboundedReceiver<moqt::object::StreamPriority>,
    publish_done: mpsc::UnboundedReceiver<SentPublishDone>,
    delivery_control_sender: watch::Sender<DeliveryControl>,
    stop_sender: Option<oneshot::Sender<PublishDoneReason>>,
    delivery_stats: Arc<DeliveryStats>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl EgressRunnerHandle {
    pub(crate) fn delivery_counters(&self) -> DeliveryCounters {
        self.delivery_stats.counters()
    }

    pub(crate) fn end_upstream(&mut self, end: PublishDoneReason) {
        let stop_sender = self.stop_sender.take().expect("upstream ends once");
        let _ = stop_sender.send(end);
    }

    /// Applies a SUBSCRIBE_UPDATE the way the directory registration does.
    pub(crate) fn apply_subscribe_update(&self, update: DeliveryControl) {
        self.delivery_control_sender.send_modify(|control| {
            control.apply_update(update);
        });
    }

    pub(crate) fn set_forward(&self, forward: bool) {
        self.delivery_control_sender
            .send_modify(|control| control.forward = forward);
    }

    pub(crate) async fn expect_publish_done(&mut self) -> SentPublishDone {
        tokio::time::timeout(RECV_TIMEOUT, self.publish_done.recv())
            .await
            .expect("egress should send PUBLISH_DONE")
            .expect("egress dropped its publisher before PUBLISH_DONE")
    }

    pub(crate) async fn assert_nothing_sent_within(&mut self, window: Duration) {
        let sent = tokio::time::timeout(window, self.sent.recv()).await;
        assert!(
            sent.is_err(),
            "egress must not open or close a downstream stream: {sent:?}"
        );
    }

    pub(crate) async fn expect_stream_priority(&mut self) -> moqt::object::StreamPriority {
        tokio::time::timeout(RECV_TIMEOUT, self.priorities.recv())
            .await
            .expect("egress should open a downstream stream")
            .expect("egress dropped its publisher before opening a stream")
    }

    pub(crate) async fn expect_sent_object(&mut self) -> DataObject {
        match tokio::time::timeout(RECV_TIMEOUT, self.sent.recv()).await {
            Ok(Some(Sent::Object(object))) => object,
            other => panic!("egress should send an object downstream, got {other:?}"),
        }
    }

    pub(crate) fn assert_no_publish_done(&mut self) {
        assert!(
            self.publish_done.try_recv().is_err(),
            "no PUBLISH_DONE should have been sent"
        );
    }
}

impl Drop for EgressRunnerHandle {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

pub(crate) struct FetchDeliveryHandle {
    sent: mpsc::UnboundedReceiver<FetchSent>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl FetchDeliveryHandle {
    pub(crate) async fn expect_objects(&mut self, expected: &[(u64, u64)]) {
        for &(group_id, object_id) in expected {
            match tokio::time::timeout(RECV_TIMEOUT, self.sent.recv()).await {
                Ok(Some(FetchSent::Object(object))) => assert_eq!(
                    (object.location.group_id, object.location.object_id),
                    (group_id, object_id),
                    "fetch objects must arrive in range order"
                ),
                other => panic!("expected fetch object {{{group_id}, {object_id}}}, got {other:?}"),
            }
        }
    }

    pub(crate) async fn expect_object(&mut self) -> moqt::object::Object {
        match tokio::time::timeout(RECV_TIMEOUT, self.sent.recv()).await {
            Ok(Some(FetchSent::Object(object))) => object,
            other => panic!("expected a fetch object, got {other:?}"),
        }
    }

    pub(crate) async fn expect_end(&mut self) -> FetchSent {
        match tokio::time::timeout(RECV_TIMEOUT, self.sent.recv()).await {
            Ok(Some(end @ (FetchSent::Closed | FetchSent::Reset(_)))) => end,
            other => panic!("expected the fetch stream to end, got {other:?}"),
        }
    }

    pub(crate) async fn assert_nothing_sent_within(&mut self, window: Duration) {
        let sent = tokio::time::timeout(window, self.sent.recv()).await;
        assert!(
            sent.is_err(),
            "fetch delivery must wait for the open group: {sent:?}"
        );
    }
}

impl Drop for FetchDeliveryHandle {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

impl RelayHarness {
    pub(crate) fn new() -> Self {
        Self::with_max_cache_duration(None)
    }

    pub(crate) fn with_max_cache_duration(max_cache_duration: Option<Duration>) -> Self {
        Self::with_cache(
            Arc::new(TrackCache::new()),
            CacheStamp {
                max_cache_duration,
                persist: false,
            },
        )
    }

    /// Ingests into `cache`, a store's cache when the test needs its disk.
    pub(crate) fn with_cache(cache: Arc<TrackCache>, stamp: CacheStamp) -> Self {
        let (session_event_sender, session_event_receiver) = mpsc::unbounded_channel();
        let (stop_sender, stop_receiver) = watch::channel(false);
        Self {
            ingest: TrackIngest {
                track_key: TrackKey::new("ns", "track"),
                publisher_session_id: PUBLISHER_SESSION_ID,
                cache,
                session_event_sender,
                stop_receiver,
                stamp,
            },
            session_event_receiver,
            stop_sender,
        }
    }

    pub(crate) fn track_key(&self) -> &TrackKey {
        &self.ingest.track_key
    }

    pub(crate) fn cache(&self) -> &TrackCache {
        &self.ingest.cache
    }

    pub(crate) async fn expect_session_event(&mut self) -> SessionEvent {
        tokio::time::timeout(RECV_TIMEOUT, self.session_event_receiver.recv())
            .await
            .expect("a session event should be reported")
            .expect("session event channel should stay open")
    }

    pub(crate) fn open_upstream_stream(&self) -> UpstreamSubgroupStream {
        UpstreamSubgroupStream::open(|receiver| {
            tokio::spawn(read_stream(self.ingest.clone(), receiver))
        })
    }

    pub(crate) async fn run_stream_ingest(&self, upstream: FailingUpstream) {
        tokio::time::timeout(
            RECV_TIMEOUT,
            accept_streams(
                self.ingest.clone(),
                Box::new(upstream),
                tracing::Span::none(),
            ),
        )
        .await
        .expect("stream ingest should end");
    }

    pub(crate) async fn run_datagram_ingest(&self, upstream: FailingUpstream) {
        tokio::time::timeout(
            RECV_TIMEOUT,
            read_datagrams(self.ingest.clone(), Box::new(upstream)),
        )
        .await
        .expect("datagram ingest should end");
    }

    pub(crate) fn is_track_malformed(&self) -> bool {
        self.ingest.cache.is_malformed()
    }

    pub(crate) fn ingest_conflicting_duplicate(&self) -> [UpstreamSubgroupStream; 2] {
        let first_stream = self.open_upstream_stream();
        first_stream.header(0);
        first_stream.object(0);
        let second_stream = self.open_upstream_stream();
        second_stream.header(0);
        second_stream.object_with_payload(Bytes::from_static(b"conflicting"));
        [first_stream, second_stream]
    }

    pub(crate) fn stop_ingest(&self) {
        self.stop_sender
            .send(true)
            .expect("stream readers should hold the stop receiver");
    }

    pub(crate) fn subscribe_subgroup_opened(&self) -> broadcast::Receiver<SubgroupRun> {
        self.ingest.cache.subscribe_subgroup_opened()
    }

    pub(crate) async fn cached_object_ids(
        &self,
        key: SubgroupKey,
    ) -> Vec<(u64, moqt::object::ObjectStatus)> {
        let mut objects = Vec::new();
        let mut cursor = 0;
        while let NextObject::Object(object) = self
            .ingest
            .cache
            .next_subgroup_object_or_wait(key, 0, cursor)
            .await
            .unwrap()
        {
            objects.push((object.object.location.object_id, object.object.status));
            cursor = object.object.location.object_id + 1;
        }
        objects
    }

    pub(crate) async fn subgroup_end_after(
        &self,
        key: SubgroupKey,
        last_object_id: u64,
    ) -> NextObject {
        tokio::time::timeout(
            RECV_TIMEOUT,
            self.ingest
                .cache
                .next_subgroup_object_or_wait(key, 0, last_object_id + 1),
        )
        .await
        .expect("subgroup should be closed, not waiting for more objects")
        .unwrap()
    }

    pub(crate) async fn start_egress(
        &self,
        largest_location: Option<moqt::object::Location>,
    ) -> EgressRunnerHandle {
        self.start_egress_with_filter(moqt::subscribe::FilterType::LargestObject, largest_location)
            .await
    }

    pub(crate) async fn start_egress_with_filter(
        &self,
        filter_type: moqt::subscribe::FilterType,
        largest_location: Option<moqt::object::Location>,
    ) -> EgressRunnerHandle {
        let (publisher, observers) = MockPublisher::channel();
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (delivery_control_sender, delivery_control_receiver) =
            watch::channel(DeliveryControl::new(true, &filter_type, largest_location));
        let (stop_sender, stop_receiver) = oneshot::channel();
        let (subscribe_ok_sender, subscribe_ok_receiver) = oneshot::channel();
        let delivery_stats = Arc::new(DeliveryStats::default());
        let runner = EgressRunner::new(
            self.ingest.track_key.clone(),
            self.ingest.cache.clone(),
            Box::new(publisher),
            make_subscription(filter_type),
            ready_sender,
            largest_location,
            DownstreamRunnerSignals {
                stop_receiver,
                delivery_control_receiver,
                delivery_stats: delivery_stats.clone(),
            },
        );
        let join_handle = tokio::spawn(async move {
            let _ = runner.run(subscribe_ok_receiver).await;
        });
        tokio::time::timeout(RECV_TIMEOUT, ready_receiver)
            .await
            .expect("egress runner should signal readiness")
            .expect("egress readiness should not be dropped")
            .expect("egress runner should start");
        let _ = subscribe_ok_sender.send(());
        EgressRunnerHandle {
            sent: observers.sent,
            priorities: observers.priorities,
            publish_done: observers.publish_done,
            delivery_control_sender,
            stop_sender: Some(stop_sender),
            delivery_stats,
            join_handle,
        }
    }

    pub(crate) fn start_fetch(
        &self,
        start_location: moqt::object::Location,
        end_location: moqt::object::Location,
    ) -> FetchDeliveryHandle {
        let (sender, sent) = MockFetchSender::channel();
        let request = EgressFetchRequest {
            subscriber_session_id: 2,
            request_id: 0,
            cache: self.ingest.cache.clone(),
            start_location,
            end_location,
            group_order: moqt::object::GroupOrder::Ascending,
        };
        let join_handle = tokio::spawn(async move { deliver_fetch(&request, &sender).await });
        FetchDeliveryHandle { sent, join_handle }
    }

    pub(crate) async fn wait_group_closed(&self, group_id: u64) {
        let cache = &self.ingest.cache;
        let whole_group = location(group_id, 0);
        let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
        while !cache.covers(whole_group, whole_group) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "group {group_id} never closed"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    pub(crate) async fn wait_track_malformed(&self) {
        let cache = &self.ingest.cache;
        tokio::time::timeout(RECV_TIMEOUT, cache.malformed_track_detected())
            .await
            .expect("track should be marked malformed");
    }

    pub(crate) async fn wait_largest_location(
        &self,
        expected: moqt::object::Location,
    ) -> moqt::object::Location {
        wait_largest_location(&self.ingest.cache, expected).await
    }
}

pub(crate) async fn wait_largest_location(
    cache: &TrackCache,
    expected: moqt::object::Location,
) -> moqt::object::Location {
    let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
    loop {
        if let Some(largest) = cache.largest_location()
            && largest >= expected
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

pub(crate) async fn session_repository_with_downstream_session(
    session_id: SessionId,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    MockPublisherObservers,
) {
    let (publisher, observers) = MockPublisher::channel();
    let repository = session_repository_with_session(
        session_id,
        Box::new(MockDownstreamSession { publisher }),
        VerifiedToken::full_access(),
    )
    .await;
    (repository, observers)
}

pub(crate) async fn receive_objects_until_close(
    egress: &mut EgressRunnerHandle,
) -> Vec<DataObject> {
    let (objects, end) = receive_objects_until_end(egress).await;
    assert!(
        matches!(end, Sent::Closed),
        "downstream stream should end with a FIN, got {end:?}"
    );
    objects
}

pub(crate) async fn receive_objects_until_end(
    egress: &mut EgressRunnerHandle,
) -> (Vec<DataObject>, Sent) {
    let mut objects = Vec::new();
    loop {
        match tokio::time::timeout(RECV_TIMEOUT, egress.sent.recv()).await {
            Ok(Some(Sent::Object(object))) => objects.push(object),
            Ok(Some(end)) => return (objects, end),
            Ok(None) => panic!(
                "egress dropped its sender after sending {} objects",
                objects.len()
            ),
            Err(_) => panic!(
                "egress stalled without closing after sending {} objects",
                objects.len()
            ),
        }
    }
}

pub(crate) fn assert_full_ordered_delivery(objects: &[DataObject]) {
    assert!(
        matches!(
            objects.first(),
            Some(DataObject::SubgroupHeader(header)) if header.group_id == 0
        ),
        "downstream stream should start with the group 0 subgroup header"
    );
    let payloads = payloads_of(objects);
    let expected: Vec<Bytes> = (0..OBJECT_COUNT).map(ordered_payload).collect();
    assert_eq!(
        payloads.len(),
        expected.len(),
        "downstream stream closed before receiving {OBJECT_COUNT} objects (got {})",
        payloads.len()
    );
    assert_eq!(payloads, expected, "objects must arrive in publish order");
}

pub(crate) fn payloads_of(objects: &[DataObject]) -> Vec<Bytes> {
    objects
        .iter()
        .filter_map(|object| match object {
            DataObject::SubgroupObject(object) if !object.payload.is_empty() => {
                Some(object.payload.clone())
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn downstream_object_ids(objects: &[DataObject]) -> Vec<u64> {
    objects
        .iter()
        .filter_map(|object| match object {
            DataObject::SubgroupObject(object) => Some(object.location.object_id),
            _ => None,
        })
        .collect()
}
