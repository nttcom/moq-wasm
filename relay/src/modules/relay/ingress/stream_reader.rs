use std::sync::Arc;

use moqt::ObjectStatus;
use tokio::{
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
};
use tracing::{Instrument, Span};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::{
        cache::{
            cached_object::{CachedObject, SubgroupStream},
            store::TrackCacheStore,
            track_cache::{LiveSubgroup, TrackCache},
        },
        notifications::{track_event::TrackEvent, track_notifier::ObjectNotifyProducerMap},
    },
    session_event::SessionEvent,
    types::{SessionId, TrackKey},
};

pub(crate) struct StreamOpened {
    pub(crate) track_key: TrackKey,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) receiver: Box<dyn StreamReceiver>,
    pub(crate) parent_span: Span,
    pub(crate) stop_receiver: watch::Receiver<bool>,
}

pub(crate) struct StreamReader {
    join_handle: JoinHandle<()>,
}

/// What the SUBGROUP_HEADER told us; the subgroup id of Type 0x12/0x13/0x1A/0x1B
/// headers is only known once the first object arrives.
struct ReceivedHeader {
    group_id: u64,
    publisher_priority: u8,
    ends_group_on_fin: bool,
    prev_object_id: Option<u64>,
}

struct SubgroupIngest<'a> {
    stream: SubgroupStream,
    live: LiveSubgroup<'a>,
    last_object_id: Option<u64>,
}

impl StreamReader {
    pub(crate) fn run(
        mut receiver: mpsc::Receiver<StreamOpened>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(cmd) = receiver.recv() => {
                        let span = tracing::info_span!(
                            parent: &cmd.parent_span,
                            "relay.dataplane.ingress.stream",
                            track_key = %cmd.track_key,
                            group_id = tracing::field::Empty,
                            subgroup_id = tracing::field::Empty,
                            end_reason = tracing::field::Empty,
                        );
                        joinset.spawn(Self::read_loop(
                            cmd.track_key,
                            cmd.publisher_session_id,
                            cmd.receiver,
                            cmd.stop_receiver,
                            cache_store.clone(),
                            object_notify_producer_map.clone(),
                            session_event_sender.clone(),
                        ).instrument(span));
                    }
                    Some(result) = joinset.join_next() => {
                        if let Err(e) = result {
                            tracing::error!("stream read task panicked: {:?}", e);
                        }
                    }
                    else => break,
                }
            }
        });
        Self { join_handle }
    }

    async fn read_loop(
        track_key: TrackKey,
        publisher_session_id: SessionId,
        mut receiver: Box<dyn StreamReceiver>,
        mut stop_receiver: watch::Receiver<bool>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) {
        let span = Span::current();
        let cache = cache_store.get_or_create(&track_key);
        let notify = object_notify_producer_map.get_or_create(&track_key);
        let mut header: Option<ReceivedHeader> = None;
        let mut ingest: Option<SubgroupIngest<'_>> = None;
        loop {
            let receive_result = tokio::select! {
                _ = stop_receiver.changed() => {
                    span.record("end_reason", "stopped");
                    tracing::info!(%track_key, "stream reader stopped");
                    return;
                }
                result = receiver.receive_object() => result,
            };

            match receive_result {
                Ok(Some(DataObject::SubgroupHeader(received))) => {
                    ingest = None;
                    span.record("group_id", received.group_id);
                    let subgroup_id = match received.subgroup_id {
                        moqt::SubgroupId::None => Some(0),
                        moqt::SubgroupId::Value(subgroup_id) => Some(subgroup_id),
                        moqt::SubgroupId::FirstObjectIdDelta => None,
                    };
                    let received = ReceivedHeader {
                        group_id: received.group_id,
                        publisher_priority: received.publisher_priority,
                        ends_group_on_fin: received.message_type.has_end_of_group(),
                        prev_object_id: None,
                    };
                    if let Some(subgroup_id) = subgroup_id {
                        ingest = Some(Self::open_subgroup(
                            &cache,
                            &notify,
                            &span,
                            received.stream(subgroup_id),
                        ));
                    }
                    header = Some(received);
                }
                Ok(Some(DataObject::SubgroupObject(field))) => {
                    let Some(header) = header.as_mut() else {
                        span.record("end_reason", "object_before_header");
                        tracing::error!(%track_key, "subgroup object received before its header");
                        return;
                    };
                    let object_id = field.resolve_object_id(header.prev_object_id);
                    header.prev_object_id = Some(object_id);
                    let ingest = ingest.get_or_insert_with(|| {
                        Self::open_subgroup(&cache, &notify, &span, header.stream(object_id))
                    });
                    let end_reason = match &field.subgroup_object {
                        moqt::SubgroupObject::Status { code, .. }
                            if *code == ObjectStatus::EndOfGroup as u64 =>
                        {
                            Some("end_of_group")
                        }
                        moqt::SubgroupObject::Status { code, .. }
                            if *code == ObjectStatus::EndOfTrack as u64 =>
                        {
                            Some("end_of_track")
                        }
                        _ => None,
                    };
                    let object = match CachedObject::from_subgroup_object(
                        &ingest.stream,
                        object_id,
                        field,
                    ) {
                        Ok(object) => object,
                        Err(error) => {
                            span.record("end_reason", "invalid_status");
                            tracing::error!(%track_key, %error, object_id, "invalid object status");
                            return;
                        }
                    };
                    if ingest.live.insert(object).is_err() {
                        span.record("end_reason", "malformed_track");
                        tracing::warn!(
                            %track_key,
                            group_id = ingest.stream.group_id,
                            object_id,
                            "malformed track detected; stopping stream ingest"
                        );
                        let _ = session_event_sender.send(SessionEvent::malformed_track_detected(
                            publisher_session_id,
                            track_key.clone(),
                        ));
                        return;
                    }
                    ingest.last_object_id = Some(object_id);
                    if let Some(end_reason) = end_reason {
                        span.record("end_reason", end_reason);
                        return;
                    }
                }
                Ok(Some(DataObject::ObjectDatagram(_))) => {
                    span.record("end_reason", "unexpected_datagram");
                    tracing::error!(%track_key, "datagram received on a subgroup stream");
                    return;
                }
                Ok(None) => {
                    // FIN: the routine end of a subgroup stream that carries
                    // no explicit end-of-group status object.
                    span.record("end_reason", "fin");
                    if let (Some(header), Some(ingest)) = (&header, &ingest)
                        && header.ends_group_on_fin
                    {
                        let end_of_group_id = ingest.last_object_id.map_or(0, |id| id + 1);
                        let _ = ingest
                            .live
                            .insert(CachedObject::end_of_group(&ingest.stream, end_of_group_id));
                    }
                    tracing::debug!(%track_key, "stream finished");
                    return;
                }
                Err(moqt::StreamReceiveError::Closed(error)) => {
                    // Transport-level interruption: RESET_STREAM or the
                    // publisher connection was lost mid-subgroup.
                    span.record("end_reason", "transport_closed");
                    tracing::info!(%track_key, %error, "stream transport closed");
                    return;
                }
                Err(moqt::StreamReceiveError::Decode(error)) => {
                    // Malformed data on the wire: a peer bug or protocol
                    // violation, unlike the two endings above.
                    span.record("end_reason", "decode_error");
                    tracing::error!(%track_key, %error, "failed to decode stream data");
                    return;
                }
            }
        }
    }

    fn open_subgroup<'a>(
        cache: &'a TrackCache,
        notify: &broadcast::Sender<TrackEvent>,
        span: &Span,
        stream: SubgroupStream,
    ) -> SubgroupIngest<'a> {
        span.record("subgroup_id", stream.subgroup_id);
        let live = cache.open_live_subgroup(stream.key());
        let _ = notify.send(TrackEvent::SubgroupOpened(stream.key()));
        SubgroupIngest {
            stream,
            live,
            last_object_id: None,
        }
    }
}

impl ReceivedHeader {
    fn stream(&self, subgroup_id: u64) -> SubgroupStream {
        SubgroupStream {
            group_id: self.group_id,
            subgroup_id,
            publisher_priority: self.publisher_priority,
        }
    }
}

impl Drop for StreamReader {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use bytes::Bytes;
    use moqt::{ObjectStatus, SubgroupId};
    use tokio::sync::oneshot;

    use super::*;
    use crate::modules::relay::tests::harness::fixtures::{
        cached_object::stream_key,
        data_object::{make_header, make_header_with, make_payload_object, make_status_object},
    };
    use crate::modules::relay::types::SubgroupKey;

    // How the scripted stream ends once all objects were consumed.
    enum TerminalOutcome {
        Fin,
        TransportClosed,
        DecodeFailed,
        Hang,
    }

    struct ScriptedStreamReceiver {
        objects: VecDeque<DataObject>,
        // Fired once all scripted objects were consumed; lets tests order a
        // stop signal after ingestion without racing the read loop.
        exhausted_sender: Option<oneshot::Sender<()>>,
        terminal: TerminalOutcome,
    }

    #[async_trait::async_trait]
    impl StreamReceiver for ScriptedStreamReceiver {
        async fn receive_object(&mut self) -> Result<Option<DataObject>, moqt::StreamReceiveError> {
            if let Some(object) = self.objects.pop_front() {
                return Ok(Some(object));
            }
            if let Some(sender) = self.exhausted_sender.take() {
                let _ = sender.send(());
            }
            match self.terminal {
                TerminalOutcome::Fin => Ok(None),
                TerminalOutcome::TransportClosed => Err(moqt::StreamReceiveError::Closed(
                    "stream reset by peer".to_string(),
                )),
                TerminalOutcome::DecodeFailed => Err(moqt::StreamReceiveError::Decode(
                    "malformed object field".to_string(),
                )),
                TerminalOutcome::Hang => {
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        }
    }

    fn payload(object_id_delta: u64) -> DataObject {
        make_payload_object(object_id_delta, Bytes::from_static(b"payload"))
    }

    const PUBLISHER_SESSION: SessionId = 1;

    struct TestEnv {
        track_key: TrackKey,
        cache_store: Arc<TrackCacheStore>,
        notify_map: Arc<ObjectNotifyProducerMap>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
        _session_event_receiver: mpsc::UnboundedReceiver<SessionEvent>,
        event_receiver: tokio::sync::broadcast::Receiver<TrackEvent>,
        stop_sender: watch::Sender<bool>,
        stop_receiver: watch::Receiver<bool>,
    }

    impl TestEnv {
        fn new() -> Self {
            let track_key = TrackKey::new("ns", "track");
            let cache_store = Arc::new(TrackCacheStore::new());
            let notify_map = Arc::new(ObjectNotifyProducerMap::new());
            let event_receiver = notify_map.get_or_create(&track_key).subscribe();
            let (stop_sender, stop_receiver) = watch::channel(false);
            let (session_event_sender, session_event_receiver) = mpsc::unbounded_channel();
            TestEnv {
                track_key,
                cache_store,
                notify_map,
                session_event_sender,
                _session_event_receiver: session_event_receiver,
                event_receiver,
                stop_sender,
                stop_receiver,
            }
        }

        fn read_loop(
            &self,
            receiver: ScriptedStreamReceiver,
        ) -> impl Future<Output = ()> + Send + 'static {
            StreamReader::read_loop(
                self.track_key.clone(),
                PUBLISHER_SESSION,
                Box::new(receiver),
                self.stop_receiver.clone(),
                self.cache_store.clone(),
                self.notify_map.clone(),
                self.session_event_sender.clone(),
            )
        }

        fn cache(&self) -> Arc<TrackCache> {
            self.cache_store.get_or_create(&self.track_key)
        }

        async fn cached_object_ids(&self, key: SubgroupKey) -> Vec<(u64, ObjectStatus)> {
            let cache = self.cache();
            let mut objects = Vec::new();
            let mut cursor = 0;
            while let Some(object) = cache.next_object_or_wait(key, cursor).await {
                objects.push((object.location.object_id, object.status));
                cursor = object.location.object_id + 1;
            }
            objects
        }

        async fn assert_subgroup_closed_after(&self, key: SubgroupKey, last_object_id: u64) {
            let cache = self.cache();
            let closed = tokio::time::timeout(
                Duration::from_secs(1),
                cache.next_object_or_wait(key, last_object_id + 1),
            )
            .await
            .expect("subgroup should be closed, not waiting for more objects");
            assert!(closed.is_none());
        }
    }

    fn scripted(objects: Vec<DataObject>, terminal: TerminalOutcome) -> ScriptedStreamReceiver {
        ScriptedStreamReceiver {
            objects: VecDeque::from(objects),
            exhausted_sender: None,
            terminal,
        }
    }

    #[tokio::test]
    async fn end_of_group_status_closes_subgroup_and_notifies() {
        // Arrange
        let mut env = TestEnv::new();
        let receiver = scripted(
            vec![
                make_header(0),
                payload(0),
                make_status_object(0, ObjectStatus::EndOfGroup),
            ],
            TerminalOutcome::Fin,
        );
        // Act
        env.read_loop(receiver).await;
        // Assert
        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::SubgroupOpened(key)) if key == stream_key(0)
        ));
        assert_eq!(
            env.cached_object_ids(stream_key(0)).await,
            vec![(0, ObjectStatus::Normal), (1, ObjectStatus::EndOfGroup)]
        );
        env.assert_subgroup_closed_after(stream_key(0), 1).await;
    }

    async fn assert_open_subgroup_closed_on(terminal: TerminalOutcome) {
        // Arrange
        let mut env = TestEnv::new();
        let receiver = scripted(vec![make_header(0), payload(0)], terminal);
        // Act
        env.read_loop(receiver).await;
        // Assert
        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::SubgroupOpened(key)) if key == stream_key(0)
        ));
        env.assert_subgroup_closed_after(stream_key(0), 0).await;
    }

    #[tokio::test]
    async fn fin_closes_open_subgroup() {
        assert_open_subgroup_closed_on(TerminalOutcome::Fin).await;
    }

    #[tokio::test]
    async fn transport_close_closes_open_subgroup() {
        assert_open_subgroup_closed_on(TerminalOutcome::TransportClosed).await;
    }

    #[tokio::test]
    async fn decode_failure_closes_open_subgroup() {
        assert_open_subgroup_closed_on(TerminalOutcome::DecodeFailed).await;
    }

    #[tokio::test]
    async fn stop_signal_closes_open_subgroup() {
        // Arrange: the stream hangs after two objects
        let env = TestEnv::new();
        let (exhausted_sender, exhausted_receiver) = oneshot::channel();
        let receiver = ScriptedStreamReceiver {
            objects: VecDeque::from([make_header(0), payload(0)]),
            exhausted_sender: Some(exhausted_sender),
            terminal: TerminalOutcome::Hang,
        };
        let read_task = tokio::spawn(env.read_loop(receiver));
        exhausted_receiver
            .await
            .expect("reader should consume all scripted objects");
        // Act
        env.stop_sender.send(true).expect("stop signal should send");
        tokio::time::timeout(Duration::from_secs(1), read_task)
            .await
            .expect("read loop should stop on signal")
            .expect("read loop should not panic");
        // Assert
        env.assert_subgroup_closed_after(stream_key(0), 0).await;
    }

    #[tokio::test]
    async fn resolves_absolute_object_ids_from_deltas() {
        // Arrange: deltas 0, 0, 1 resolve to absolute ids 0, 1, 3
        let env = TestEnv::new();
        let receiver = scripted(
            vec![make_header(0), payload(0), payload(0), payload(1)],
            TerminalOutcome::Fin,
        );
        // Act
        env.read_loop(receiver).await;
        // Assert
        let ids: Vec<u64> = env
            .cached_object_ids(stream_key(0))
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec![0, 1, 3]);
    }

    #[tokio::test]
    async fn end_of_group_header_type_synthesizes_status_object_on_fin() {
        // Arrange: Type 0x18 header (last object before FIN ends the group)
        let env = TestEnv::new();
        let receiver = scripted(
            vec![
                make_header_with(0, SubgroupId::None, true),
                payload(0),
                payload(0),
            ],
            TerminalOutcome::Fin,
        );
        // Act
        env.read_loop(receiver).await;
        // Assert: the End of Group becomes canonical data at last_id + 1
        assert_eq!(
            env.cached_object_ids(stream_key(0)).await,
            vec![
                (0, ObjectStatus::Normal),
                (1, ObjectStatus::Normal),
                (2, ObjectStatus::EndOfGroup)
            ]
        );
    }

    #[tokio::test]
    async fn end_of_group_header_type_does_not_synthesize_on_reset() {
        // Arrange: a reset stream cannot tell where the group ended (§10.4.2)
        let env = TestEnv::new();
        let receiver = scripted(
            vec![make_header_with(0, SubgroupId::None, true), payload(0)],
            TerminalOutcome::TransportClosed,
        );
        // Act
        env.read_loop(receiver).await;
        // Assert
        assert_eq!(
            env.cached_object_ids(stream_key(0)).await,
            vec![(0, ObjectStatus::Normal)]
        );
    }

    #[tokio::test]
    async fn first_object_id_delta_header_takes_subgroup_id_from_first_object() {
        // Arrange: Type 0x12 header; the first object arrives with id 5
        let mut env = TestEnv::new();
        let receiver = scripted(
            vec![
                make_header_with(0, SubgroupId::FirstObjectIdDelta, false),
                payload(5),
                payload(0),
            ],
            TerminalOutcome::Fin,
        );
        // Act
        env.read_loop(receiver).await;
        // Assert: the subgroup is opened, and announced, only once its id is known
        let key = SubgroupKey::Stream {
            group_id: 0,
            subgroup_id: 5,
        };
        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::SubgroupOpened(opened)) if opened == key
        ));
        let ids: Vec<u64> = env
            .cached_object_ids(key)
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec![5, 6]);
    }
}
