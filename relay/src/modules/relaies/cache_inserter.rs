use std::sync::Arc;

use crate::modules::relaies::{
    cache_repository::CacheRepository, caches::cache::Cache, receiver_monitor::ReceivedData,
};

pub(crate) struct CacheInserter {
    join_handle: tokio::task::JoinHandle<()>,
}

impl CacheInserter {
    pub(crate) fn run(
        mut receiver: tokio::sync::mpsc::Receiver<ReceivedData>,
        cache_repository: Arc<CacheRepository>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            while let Some(received_data) = receiver.recv().await {
                match received_data {
                    ReceivedData::Data {
                        track_key,
                        group_id,
                        data_object,
                    } => {
                        let cache = cache_repository.get_cache(&track_key);
                        cache
                            .set_latest_object(track_key, group_id, data_object)
                            .await;
                    }
                    ReceivedData::EndOfStream {
                        track_key,
                        group_id,
                    } => {
                        let cache = cache_repository.get_cache(&track_key);
                        cache.notify_end_of_stream(track_key, group_id).await;
                    }
                }
            }
        });
        CacheInserter { join_handle }
    }
}
