use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::datagram_receiver::DatagramReceiver,
    relay::{
        cache::store::TrackCacheStore,
        notifications::{track_event::TrackEvent, sender_map::SenderMap},
    },
    types::TrackKey,
};

pub(crate) struct DatagramReceiveStart {
    pub(crate) track_key: TrackKey,
    pub(crate) receiver: Box<dyn DatagramReceiver>,
}

pub(crate) struct DatagramReader {
    join_handle: JoinHandle<()>,
}

impl DatagramReader {
    pub(crate) fn run(
        mut receiver: mpsc::Receiver<DatagramReceiveStart>,
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
                            tracing::error!("datagram read task panicked: {:?}", e);
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
        mut receiver: Box<dyn DatagramReceiver>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<SenderMap>,
    ) {
        let mut current_group_id: Option<u64> = None;
        loop {
            match receiver.receive_object().await {
                Ok(object) => {
                    let group_id = object.group_id().or(current_group_id).unwrap_or(0);
                    if current_group_id != Some(group_id) {
                        if let Some(old_group) = current_group_id {
                            let cache = cache_store.get_or_create(track_key);
                            cache.close_group(old_group).await;
                        }
                        current_group_id = Some(group_id);
                        let _ = sender_map
                            .get_or_create(track_key)
                            .send(TrackEvent::DatagramOpened { group_id });
                    }
                    let cache = cache_store.get_or_create(track_key);
                    cache.append_object(group_id, object).await;
                    let _ = sender_map
                        .get_or_create(track_key)
                        .send(TrackEvent::LatestObject);
                }
                Err(_) => {
                    // Ensure the last group is closed before exiting.
                    if let Some(group_id) = current_group_id {
                        let cache = cache_store.get_or_create(track_key);
                        cache.close_group(group_id).await;
                    }
                    tracing::debug!(track_key, "datagram receiver ended");
                    return;
                }
            }
        }
    }
}

impl Drop for DatagramReader {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
