use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::{
    core::data_object::DataObject,
    enums::Location,
    relay::caches::{cache::Cache, group_of_frames_map::GroupOfFramesMap, latest_info::LatestInfo},
};

pub(crate) struct CacheMap {
    pub(crate) caches: DashMap<u64, GroupOfFramesMap>,
}

impl CacheMap {
    pub(crate) fn new() -> Self {
        let sender = tokio::sync::broadcast::channel(128).0;
        Self {
            caches: DashMap::new(),
            latest_object_notifier: sender,
        }
    }
}

#[async_trait::async_trait]
impl Cache for CacheMap {
    async fn add_object(&self, track_key: u128, group_id: u64, object: DataObject) -> u64 {
        let cache = self
            .caches
            .entry(group_id)
            .or_insert_with(GroupOfFramesMap::new);
        let object = Arc::new(object);
        cache.frames.write().await.push(object.clone());
        let index = cache.frames.read().await.len() as u64 - 1;
        index
    }

    async fn get_group(&self, group_id: u64) -> Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>> {
        self.caches.get(&group_id).unwrap().value().frames.clone()
    }
}
