use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::relay::{
    cache::store::TrackCacheStore,
    ingest::received_event::ReceivedEvent,
};

pub(crate) struct CacheWriter {
    sender: mpsc::Sender<ReceivedEvent>,
    _join_handle: JoinHandle<()>,
}

impl CacheWriter {
    pub(crate) fn start(cache_store: Arc<TrackCacheStore>, queue_capacity: usize) -> Self {
        let (sender, mut receiver) = mpsc::channel::<ReceivedEvent>(queue_capacity);
        let join_handle = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                match event {
                    ReceivedEvent::StreamOpened {
                        track_key,
                        group_id,
                    } => {
                        let cache = cache_store.get_or_create(track_key);
                        cache.ensure_group(group_id).await;
                    }
                    ReceivedEvent::Object {
                        track_key,
                        group_id,
                        object,
                    } => {
                        let cache = cache_store.get_or_create(track_key);
                        cache.append_object(track_key, group_id, object).await;
                    }
                    ReceivedEvent::EndOfGroup {
                        track_key,
                        group_id,
                    } => {
                        let cache = cache_store.get_or_create(track_key);
                        cache.close_group(track_key, group_id).await;
                    }
                    ReceivedEvent::DatagramClosed { track_key } => {
                        tracing::debug!(track_key, "datagram reader closed");
                    }
                }
            }
        });

        Self {
            sender,
            _join_handle: join_handle,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<ReceivedEvent> {
        self.sender.clone()
    }
}