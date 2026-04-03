use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::{
        cache::store::TrackCacheStore,
        notifications::{latest_info::LatestInfo, sender_map::SenderMap},
    },
    types::TrackKey,
};

pub(crate) struct StreamReceiveStart {
    pub(crate) track_key: TrackKey,
    pub(crate) receiver: Box<dyn StreamReceiver>,
}

pub(crate) struct StreamIngestTask {
    join_handle: JoinHandle<()>,
}

impl StreamIngestTask {
    pub(crate) fn new(
        mut receiver: mpsc::Receiver<StreamReceiveStart>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<SenderMap>,
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
        sender_map: Arc<SenderMap>,
    ) {
        let mut group_id = 0u64;
        loop {
            match receiver.receive_object().await {
                Ok(DataObject::SubgroupHeader(header)) => {
                    group_id = header.group_id;
                    let cache = cache_store.get_or_create(track_key);
                    cache
                        .append_object(group_id, DataObject::SubgroupHeader(header))
                        .await;
                    let _ = sender_map
                        .get_or_create(track_key)
                        .send(LatestInfo::StreamOpened { group_id });
                }
                Ok(object) => {
                    let cache = cache_store.get_or_create(track_key);
                    cache.append_object(group_id, object).await;
                    let _ = sender_map
                        .get_or_create(track_key)
                        .send(LatestInfo::LatestObject);
                }
                Err(_) => {
                    let cache = cache_store.get_or_create(track_key);
                    cache.close_group(group_id).await;
                    let _ = sender_map
                        .get_or_create(track_key)
                        .send(LatestInfo::EndOfGroup);
                    return;
                }
            }
        }
    }
}

impl Drop for StreamIngestTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
