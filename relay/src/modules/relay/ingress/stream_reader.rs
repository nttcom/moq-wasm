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
                Ok(DataObject::SubgroupHeader(header)) => {
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
                Ok(object) => {
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
                Err(_) => {
                    span.record("end_reason", "receiver_error");
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
