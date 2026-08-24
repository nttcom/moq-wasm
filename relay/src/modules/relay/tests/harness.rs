use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot, watch};

use crate::modules::{
    core::data_object::DataObject,
    relay::{
        cache::store::TrackCacheStore,
        egress::runner::EgressRunner,
        ingress::stream_reader::{StreamOpened, StreamReader},
        notifications::track_notifier::ObjectNotifyProducerMap,
    },
    types::TrackKey,
};

mod fixtures;
mod mocks;

use self::{
    fixtures::{data_object::ordered_payload, subscription::make_largest_object_subscription},
    mocks::{downstream_client::MockPublisher, upstream_client::UpstreamSubgroupStream},
};

pub(crate) const OBJECT_COUNT: usize = 50;
const RECV_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct RelayHarness {
    track_key: TrackKey,
    cache_store: Arc<TrackCacheStore>,
    notify_map: Arc<ObjectNotifyProducerMap>,
    opened_sender: mpsc::Sender<StreamOpened>,
    _stream_reader: StreamReader,
    _stop_sender: watch::Sender<bool>,
    stop_receiver: watch::Receiver<bool>,
}

pub(crate) struct EgressRunnerHandle {
    sent: mpsc::UnboundedReceiver<Option<DataObject>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl Drop for EgressRunnerHandle {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

impl RelayHarness {
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
            cache_store,
            notify_map,
            opened_sender,
            _stream_reader: stream_reader,
            _stop_sender: stop_sender,
            stop_receiver,
        }
    }

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
            .expect("stream reader should accept new streams");
        upstream_stream
    }

    pub(crate) async fn start_egress(
        &self,
        largest_location: Option<moqt::Location>,
    ) -> EgressRunnerHandle {
        let (publisher, sent) = MockPublisher::channel();
        let (ready_sender, ready_receiver) = oneshot::channel();
        let runner = EgressRunner::new(
            self.track_key.clone(),
            self.cache_store.get_or_create(&self.track_key),
            self.notify_map.get_or_create(&self.track_key),
            Box::new(publisher),
            make_largest_object_subscription(),
            ready_sender,
            largest_location,
        );
        let join_handle = tokio::spawn(async move {
            let _ = runner.run().await;
        });
        tokio::time::timeout(RECV_TIMEOUT, ready_receiver)
            .await
            .expect("egress runner should signal readiness")
            .expect("egress readiness should not be dropped")
            .expect("egress runner should start");
        EgressRunnerHandle { sent, join_handle }
    }

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

pub(crate) async fn receive_objects_until_close(
    egress: &mut EgressRunnerHandle,
) -> Vec<DataObject> {
    let mut objects = Vec::new();
    loop {
        match tokio::time::timeout(RECV_TIMEOUT, egress.sent.recv()).await {
            Ok(Some(Some(object))) => objects.push(object),
            Ok(Some(None)) => return objects,
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
    let payloads: Vec<Bytes> = objects
        .iter()
        .filter_map(|object| match object {
            DataObject::SubgroupObject(field) => match &field.subgroup_object {
                moqt::SubgroupObject::Payload { data, .. } => Some(data.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let expected: Vec<Bytes> = (0..OBJECT_COUNT).map(ordered_payload).collect();
    assert_eq!(
        payloads.len(),
        expected.len(),
        "downstream stream closed before receiving {OBJECT_COUNT} objects (got {})",
        payloads.len()
    );
    assert_eq!(payloads, expected, "objects must arrive in publish order");
}
