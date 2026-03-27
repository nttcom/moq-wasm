use std::sync::Arc;

use crate::modules::{
    core::data_receiver::receiver,
    enums::FilterType,
    relaies::{cache_repository::CacheRepository, receiver_monitor::ReceivedData},
    types::TrackKey,
};

pub(crate) struct CachePicker {
    join_handle: tokio::task::JoinHandle<()>,
}

impl CachePicker {
    pub(crate) fn run(
        track_key: TrackKey,
        cache_repo: Arc<CacheRepository>,
        filter_type: FilterType,
        receiver: tokio::sync::broadcast::Receiver<ReceivedData>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            match filter_type {
                FilterType::LatestObject => {
                    let cache = cache_repo.get_cache(&track_key);
                    // Use the cache to get the latest data object for the track_key
                }
                FilterType::LatestGroup => {
                    let cache = cache_repo.get_cache(&track_key);
                    // Use the cache to get all data objects for the track_key
                }
                FilterType::AbsoluteStart { location } => {
                    let cache = cache_repo.get_cache(&track_key);
                    // Use the cache to get data objects starting from the absolute location
                }
                FilterType::AbsoluteRange {
                    location,
                    end_group,
                } => {
                    let cache = cache_repo.get_cache(&track_key);
                    // Use the cache to get data objects starting from the relative offset
                }
            }
        });
        CachePicker { join_handle }
    }
}
