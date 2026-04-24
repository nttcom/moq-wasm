use std::sync::Arc;

use moqt::wire::ObjectStatus;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::{
        cache::store::TrackCacheStore,
        notifications::{track_event::TrackEvent, track_notifier::TrackNotifier},
        types::StreamSubgroupId,
    },
    types::TrackKey,
};

pub(crate) struct StreamOpened {
    pub(crate) track_key: TrackKey,
    pub(crate) receiver: Box<dyn StreamReceiver>,
}

pub(crate) struct StreamReader {
    join_handle: JoinHandle<()>,
}

impl StreamReader {
    pub(crate) fn run(
        mut receiver: mpsc::Receiver<StreamOpened>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<TrackNotifier>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(cmd) = receiver.recv() => {
                        joinset.spawn(Self::read_loop(
                            cmd.track_key,
                            cmd.receiver,
                            cache_store.clone(),
                            sender_map.clone(),
                        ));
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
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<TrackNotifier>,
    ) {
        let mut group_id = 0u64;
        let mut subgroup_id = StreamSubgroupId::None;
        loop {
            match receiver.receive_object().await {
                Ok(DataObject::SubgroupHeader(header)) => {
                    group_id = header.group_id;
                    subgroup_id = StreamSubgroupId::from(&header.subgroup_id);
                    let cache = cache_store.get_or_create(track_key);
                    cache
                        .append_stream_object(
                            group_id,
                            &subgroup_id,
                            DataObject::SubgroupHeader(header),
                        )
                        .await;
                    let _ = sender_map
                        .get_or_create(track_key)
                        .send(TrackEvent::StreamOpened {
                            group_id,
                            subgroup_id: subgroup_id.clone(),
                        });
                }
                Ok(object) => {
                    let should_close = matches!(
                        &object,
                        DataObject::SubgroupObject(field)
                            if matches!(
                                &field.subgroup_object,
                                moqt::SubgroupObject::Status { code, .. }
                                    if *code == ObjectStatus::EndOfGroup as u64
                                        || *code == ObjectStatus::EndOfTrack as u64
                            )
                    );
                    let cache = cache_store.get_or_create(track_key);
                    cache
                        .append_stream_object(group_id, &subgroup_id, object)
                        .await;
                    if should_close {
                        cache.close_stream_subgroup(group_id, &subgroup_id).await;
                        let _ = sender_map
                            .get_or_create(track_key)
                            .send(TrackEvent::EndOfGroup);
                        return;
                    }
                }
                Err(_) => {
                    let cache = cache_store.get_or_create(track_key);
                    cache.close_stream_subgroup(group_id, &subgroup_id).await;
                    let _ = sender_map
                        .get_or_create(track_key)
                        .send(TrackEvent::EndOfGroup);
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
