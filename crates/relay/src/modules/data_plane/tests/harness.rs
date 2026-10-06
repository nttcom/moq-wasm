use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::modules::{
    auth::verified_token::VerifiedToken,
    core::{data_object::DataObject, mocks::session_repository_with_session},
    data_plane::{
        cache::subgroup_key::SubgroupKey,
        cache::track_cache::{NextObject, SubgroupRun, TrackCache},
        egress::{
            coordinator::EgressFetchRequest, fetch_delivery::deliver_fetch, runner::EgressRunner,
        },
        ingress::{stream_reader::read_stream, track_ingest_task::TrackIngest},
    },
    sequences::tables::table::PublishDoneReason,
    session_event::SessionEvent,
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
};

pub(crate) mod fixtures;
mod mocks;

pub(crate) use self::mocks::downstream_client::{FetchSent, MockPublisherObservers, Sent};
pub(crate) use self::mocks::upstream_client::UpstreamSubgroupStream;

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
    priorities: mpsc::UnboundedReceiver<moqt::StreamPriority>,
    publish_done: mpsc::UnboundedReceiver<SentPublishDone>,
    forward_sender: watch::Sender<bool>,
    stop_sender: Option<oneshot::Sender<PublishDoneReason>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl EgressRunnerHandle {
    pub(crate) fn end_upstream(&mut self, end: PublishDoneReason) {
        let stop_sender = self.stop_sender.take().expect("upstream ends once");
        let _ = stop_sender.send(end);
    }

    pub(crate) fn set_forward(&self, forward: bool) {
        self.forward_sender.send_replace(forward);
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

    pub(crate) async fn expect_stream_priority(&mut self) -> moqt::StreamPriority {
        tokio::time::timeout(RECV_TIMEOUT, self.priorities.recv())
            .await
            .expect("egress should open a downstream stream")
            .expect("egress dropped its publisher before opening a stream")
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
                    (object.group_id, object.object_id),
                    (group_id, object_id),
                    "fetch objects must arrive in range order"
                ),
                other => panic!("expected fetch object {{{group_id}, {object_id}}}, got {other:?}"),
            }
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
        let (session_event_sender, session_event_receiver) = mpsc::unbounded_channel();
        let (stop_sender, stop_receiver) = watch::channel(false);
        Self {
            ingest: TrackIngest {
                track_key: TrackKey::new("ns", "track"),
                publisher_session_id: PUBLISHER_SESSION_ID,
                cache: Arc::new(TrackCache::new()),
                session_event_sender,
                stop_receiver,
            },
            session_event_receiver,
            stop_sender,
        }
    }

    pub(crate) fn track_key(&self) -> &TrackKey {
        &self.ingest.track_key
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
    ) -> Vec<(u64, moqt::ObjectStatus)> {
        let mut objects = Vec::new();
        let mut cursor = 0;
        while let NextObject::Object(object) = self
            .ingest
            .cache
            .next_subgroup_object_or_wait(key, 0, cursor)
            .await
            .unwrap()
        {
            objects.push((object.location.object_id, object.status));
            cursor = object.location.object_id + 1;
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
        largest_location: Option<moqt::Location>,
    ) -> EgressRunnerHandle {
        self.start_egress_with_filter(moqt::FilterType::LargestObject, largest_location)
            .await
    }

    pub(crate) async fn start_egress_with_filter(
        &self,
        filter_type: moqt::FilterType,
        largest_location: Option<moqt::Location>,
    ) -> EgressRunnerHandle {
        let (publisher, observers) = MockPublisher::channel();
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (forward_sender, forward_receiver) = watch::channel(true);
        let (stop_sender, stop_receiver) = oneshot::channel();
        let (subscribe_ok_sender, subscribe_ok_receiver) = oneshot::channel();
        let runner = EgressRunner::new(
            self.ingest.track_key.clone(),
            self.ingest.cache.clone(),
            Box::new(publisher),
            make_subscription(filter_type),
            ready_sender,
            largest_location,
            forward_receiver,
        );
        let join_handle = tokio::spawn(async move {
            let _ = runner.run(stop_receiver, subscribe_ok_receiver).await;
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
            forward_sender,
            stop_sender: Some(stop_sender),
            join_handle,
        }
    }

    pub(crate) fn start_fetch(
        &self,
        start_location: moqt::Location,
        end_location: moqt::Location,
    ) -> FetchDeliveryHandle {
        let (sender, sent) = MockFetchSender::channel();
        let request = EgressFetchRequest {
            subscriber_session_id: 2,
            request_id: 0,
            cache: self.ingest.cache.clone(),
            start_location,
            end_location,
            group_order: moqt::GroupOrder::Ascending,
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

    pub(crate) async fn wait_largest_location(&self, expected: moqt::Location) -> moqt::Location {
        let cache = &self.ingest.cache;
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
            DataObject::SubgroupObject(field) => match &field.subgroup_object {
                moqt::SubgroupObject::Payload { data, .. } => Some(data.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

pub(crate) fn resolve_downstream_object_ids(objects: &[DataObject]) -> Vec<u64> {
    let mut prev_object_id = None;
    objects
        .iter()
        .filter_map(|object| match object {
            DataObject::SubgroupObject(field) => {
                let object_id = field.resolve_object_id(prev_object_id);
                prev_object_id = Some(object_id);
                Some(object_id)
            }
            DataObject::SubgroupHeader(_) => {
                prev_object_id = None;
                None
            }
            DataObject::ObjectDatagram(_) => None,
        })
        .collect()
}
