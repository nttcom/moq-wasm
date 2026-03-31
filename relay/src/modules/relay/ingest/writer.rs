use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::relay::{
    cache::store::TrackCacheStore,
    caches::{latest_info::LatestInfo, sender_map::SenderMap},
    ingest::received_event::ReceivedEvent,
};

pub(crate) struct CacheWriter {
    sender: mpsc::Sender<ReceivedEvent>,
    join_handle: JoinHandle<()>,
}

impl CacheWriter {
    pub(crate) fn start(
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<SenderMap>,
        queue_capacity: usize,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<ReceivedEvent>(queue_capacity);
        let join_handle =
            tokio::spawn(async move {
                while let Some(event) = receiver.recv().await {
                    match event {
                        ReceivedEvent::StreamOpened {
                            track_key,
                            group_id,
                            object,
                        } => {
                            let cache = cache_store.get_or_create(track_key);
                            cache.append_object(track_key, group_id, object).await;
                            let _ = sender_map.get_or_create(track_key).send(
                                LatestInfo::StreamOpened {
                                    track_key,
                                    group_id,
                                },
                            );
                        }
                        ReceivedEvent::DatagramOpened {
                            track_key,
                            group_id,
                        } => {
                            let _ = sender_map.get_or_create(track_key).send(
                                LatestInfo::DatagramOpened {
                                    track_key,
                                    group_id,
                                },
                            );
                        }
                        ReceivedEvent::Object {
                            track_key,
                            group_id,
                            object,
                        } => {
                            let cache = cache_store.get_or_create(track_key);
                            let offset = cache.append_object(track_key, group_id, object).await;
                            let _ = sender_map.get_or_create(track_key).send(
                                LatestInfo::LatestObject {
                                    track_key,
                                    group_id,
                                    offset,
                                },
                            );
                        }
                        ReceivedEvent::EndOfGroup {
                            track_key,
                            group_id,
                        } => {
                            let _ =
                                sender_map
                                    .get_or_create(track_key)
                                    .send(LatestInfo::EndOfGroup {
                                        track_key,
                                        group_id,
                                    });
                        }
                        ReceivedEvent::DatagramClosed { track_key } => {
                            tracing::debug!(track_key, "datagram reader closed");
                        }
                    }
                }
            });

        Self {
            sender,
            join_handle,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<ReceivedEvent> {
        self.sender.clone()
    }
}

impl Drop for CacheWriter {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
