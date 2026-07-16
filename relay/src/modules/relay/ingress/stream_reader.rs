use std::sync::Arc;

use moqt::wire::ObjectStatus;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::{Instrument, Span};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::{
        cache::store::TrackCacheStore,
        notifications::{track_event::TrackEvent, track_notifier::ObjectNotifyProducerMap},
        types::StreamSubgroupId,
    },
    types::TrackKey,
};

pub(crate) struct StreamOpened {
    pub(crate) track_key: TrackKey,
    pub(crate) receiver: Box<dyn StreamReceiver>,
    pub(crate) parent_span: Span,
    pub(crate) stop_receiver: watch::Receiver<bool>,
}

pub(crate) struct StreamReader {
    join_handle: JoinHandle<()>,
}

impl StreamReader {
    pub(crate) fn run(
        mut receiver: mpsc::Receiver<StreamOpened>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
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
                            cmd.receiver,
                            cmd.stop_receiver,
                            cache_store.clone(),
                            object_notify_producer_map.clone(),
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
        mut receiver: Box<dyn StreamReceiver>,
        mut stop_receiver: watch::Receiver<bool>,
        cache_store: Arc<TrackCacheStore>,
        object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
    ) {
        let span = Span::current();
        let mut group_id = 0u64;
        let mut subgroup_id = StreamSubgroupId::None;
        let mut has_subgroup = false;
        let mut prev_object_id: Option<u64> = None;
        let cache = cache_store.get_or_create(&track_key);
        let notify = object_notify_producer_map.get_or_create(&track_key);
        loop {
            let receive_result = tokio::select! {
                _ = stop_receiver.changed() => {
                    span.record("end_reason", "stopped");
                    if has_subgroup {
                        cache.close_stream_subgroup(group_id, &subgroup_id).await;
                        let _ = notify.send(TrackEvent::EndOfGroup);
                    }
                    tracing::info!(%track_key, "stream reader stopped");
                    return;
                }
                result = receiver.receive_object() => result,
            };

            match receive_result {
                Ok(Some(DataObject::SubgroupHeader(header))) => {
                    group_id = header.group_id;
                    subgroup_id = StreamSubgroupId::from(&header.subgroup_id);
                    has_subgroup = true;
                    prev_object_id = None;
                    span.record("group_id", group_id);
                    span.record("subgroup_id", tracing::field::debug(&subgroup_id));
                    cache
                        .append_stream_object(
                            group_id,
                            &subgroup_id,
                            None,
                            DataObject::SubgroupHeader(header),
                        )
                        .await;
                    let _ = notify.send(TrackEvent::StreamOpened {
                        group_id,
                        subgroup_id: subgroup_id.clone(),
                    });
                }
                Ok(Some(object)) => {
                    let end_reason = match &object {
                        DataObject::SubgroupObject(field) => match &field.subgroup_object {
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
                        },
                        _ => None,
                    };
                    let object_id = object.resolve_absolute_object_id(prev_object_id);
                    prev_object_id = object_id;
                    cache
                        .append_stream_object(group_id, &subgroup_id, object_id, object)
                        .await;
                    if let Some(end_reason) = end_reason {
                        span.record("end_reason", end_reason);
                        cache.close_stream_subgroup(group_id, &subgroup_id).await;
                        let _ = notify.send(TrackEvent::EndOfGroup);
                        return;
                    }
                }
                Ok(None) => {
                    // FIN: the routine end of a subgroup stream that carries
                    // no explicit end-of-group status object.
                    span.record("end_reason", "fin");
                    if has_subgroup {
                        cache.close_stream_subgroup(group_id, &subgroup_id).await;
                        let _ = notify.send(TrackEvent::EndOfGroup);
                    }
                    tracing::debug!(%track_key, "stream finished");
                    return;
                }
                Err(moqt::StreamReceiveError::Closed(error)) => {
                    // Transport-level interruption: RESET_STREAM or the
                    // publisher connection was lost mid-subgroup.
                    span.record("end_reason", "transport_closed");
                    tracing::info!(%track_key, %error, "stream transport closed");
                    if has_subgroup {
                        cache.close_stream_subgroup(group_id, &subgroup_id).await;
                        let _ = notify.send(TrackEvent::EndOfGroup);
                    }
                    return;
                }
                Err(moqt::StreamReceiveError::Decode(error)) => {
                    // Malformed data on the wire: a peer bug or protocol
                    // violation, unlike the two endings above.
                    span.record("end_reason", "decode_error");
                    tracing::error!(%track_key, %error, "failed to decode stream data");
                    if has_subgroup {
                        cache.close_stream_subgroup(group_id, &subgroup_id).await;
                        let _ = notify.send(TrackEvent::EndOfGroup);
                    }
                    return;
                }
            }
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
    use moqt::{
        ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField,
        wire::ObjectStatus,
    };
    use tokio::sync::oneshot;

    use super::*;

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

    fn make_header(group_id: u64) -> DataObject {
        DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            group_id,
            SubgroupId::Value(0),
            0,
            false,
            false,
        ))
    }

    fn empty_extension_headers() -> ExtensionHeaders {
        ExtensionHeaders::default()
    }

    fn make_payload_object(object_id_delta: u64) -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta,
            extension_headers: empty_extension_headers(),
            subgroup_object: SubgroupObject::new_payload(Bytes::from(vec![1])),
        })
    }

    fn make_end_of_group_object() -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: empty_extension_headers(),
            subgroup_object: SubgroupObject::new_status(ObjectStatus::EndOfGroup as u64),
        })
    }

    struct TestEnv {
        track_key: TrackKey,
        cache_store: Arc<TrackCacheStore>,
        notify_map: Arc<ObjectNotifyProducerMap>,
        event_receiver: tokio::sync::broadcast::Receiver<TrackEvent>,
        stop_sender: watch::Sender<bool>,
        stop_receiver: watch::Receiver<bool>,
    }

    fn make_env() -> TestEnv {
        let track_key = TrackKey::new("ns", "track");
        let cache_store = Arc::new(TrackCacheStore::new());
        let notify_map = Arc::new(ObjectNotifyProducerMap::new());
        let event_receiver = notify_map.get_or_create(&track_key).subscribe();
        let (stop_sender, stop_receiver) = watch::channel(false);
        TestEnv {
            track_key,
            cache_store,
            notify_map,
            event_receiver,
            stop_sender,
            stop_receiver,
        }
    }

    async fn assert_subgroup_closed_after(env: &TestEnv, group_id: u64, last_object_id: u64) {
        let cache = env.cache_store.get_or_create(&env.track_key);
        let subgroup = StreamSubgroupId::Value(0);
        let closed = tokio::time::timeout(
            Duration::from_secs(1),
            cache.stream_object_from_or_wait(group_id, &subgroup, last_object_id + 1),
        )
        .await
        .expect("subgroup should be closed, not waiting for more objects");
        assert!(closed.is_none());
    }

    #[tokio::test]
    async fn end_of_group_status_closes_subgroup_and_notifies() {
        let mut env = make_env();
        let receiver = ScriptedStreamReceiver {
            objects: VecDeque::from([
                make_header(0),
                make_payload_object(0),
                make_end_of_group_object(),
            ]),
            exhausted_sender: None,
            terminal: TerminalOutcome::Fin,
        };

        StreamReader::read_loop(
            env.track_key.clone(),
            Box::new(receiver),
            env.stop_receiver.clone(),
            env.cache_store.clone(),
            env.notify_map.clone(),
        )
        .await;

        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::StreamOpened { group_id: 0, .. })
        ));
        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::EndOfGroup)
        ));

        let cache = env.cache_store.get_or_create(&env.track_key);
        let subgroup = StreamSubgroupId::Value(0);
        let (payload_id, _) = cache
            .stream_object_from_or_wait(0, &subgroup, 0)
            .await
            .expect("payload object should be cached");
        assert_eq!(payload_id, 0);
        let (status_id, _) = cache
            .stream_object_from_or_wait(0, &subgroup, 1)
            .await
            .expect("end-of-group status object should be cached");
        assert_eq!(status_id, 1);
        assert_subgroup_closed_after(&env, 0, 1).await;
    }

    // Runs read_loop over [header, one payload object] ending with `terminal`,
    // then asserts the open subgroup was closed and EndOfGroup notified.
    async fn assert_open_subgroup_closed_on(terminal: TerminalOutcome) {
        let mut env = make_env();
        let receiver = ScriptedStreamReceiver {
            objects: VecDeque::from([make_header(0), make_payload_object(0)]),
            exhausted_sender: None,
            terminal,
        };

        StreamReader::read_loop(
            env.track_key.clone(),
            Box::new(receiver),
            env.stop_receiver.clone(),
            env.cache_store.clone(),
            env.notify_map.clone(),
        )
        .await;

        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::StreamOpened { group_id: 0, .. })
        ));
        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::EndOfGroup)
        ));
        assert_subgroup_closed_after(&env, 0, 0).await;
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
        let mut env = make_env();
        let (exhausted_sender, exhausted_receiver) = oneshot::channel();
        let receiver = ScriptedStreamReceiver {
            objects: VecDeque::from([make_header(0), make_payload_object(0)]),
            exhausted_sender: Some(exhausted_sender),
            terminal: TerminalOutcome::Hang,
        };

        let read_task = tokio::spawn(StreamReader::read_loop(
            env.track_key.clone(),
            Box::new(receiver),
            env.stop_receiver.clone(),
            env.cache_store.clone(),
            env.notify_map.clone(),
        ));

        exhausted_receiver
            .await
            .expect("reader should consume all scripted objects");
        env.stop_sender.send(true).expect("stop signal should send");
        tokio::time::timeout(Duration::from_secs(1), read_task)
            .await
            .expect("read loop should stop on signal")
            .expect("read loop should not panic");

        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::StreamOpened { group_id: 0, .. })
        ));
        assert!(matches!(
            env.event_receiver.try_recv(),
            Ok(TrackEvent::EndOfGroup)
        ));
        assert_subgroup_closed_after(&env, 0, 0).await;
    }

    #[tokio::test]
    async fn resolves_absolute_object_ids_from_deltas() {
        let mut env = make_env();
        let receiver = ScriptedStreamReceiver {
            // deltas 0, 0, 1 resolve to absolute ids 0, 1, 3
            objects: VecDeque::from([
                make_header(0),
                make_payload_object(0),
                make_payload_object(0),
                make_payload_object(1),
            ]),
            exhausted_sender: None,
            terminal: TerminalOutcome::Fin,
        };

        StreamReader::read_loop(
            env.track_key.clone(),
            Box::new(receiver),
            env.stop_receiver.clone(),
            env.cache_store.clone(),
            env.notify_map.clone(),
        )
        .await;
        // silence unused warnings for the fields this test does not exercise
        let _ = (&env.stop_sender, &mut env.event_receiver);

        let cache = env.cache_store.get_or_create(&env.track_key);
        let subgroup = StreamSubgroupId::Value(0);
        let mut object_ids = Vec::new();
        let mut cursor = 0;
        while let Some((id, _)) = cache.stream_object_from_or_wait(0, &subgroup, cursor).await {
            object_ids.push(id);
            cursor = id + 1;
        }
        assert_eq!(object_ids, vec![0, 1, 3]);
    }
}
