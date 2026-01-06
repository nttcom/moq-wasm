use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::{
    core::data_object::DataObject,
    relaies::caches::{
        cache::Cache, group_of_frames_map::GroupOfFramesMap, latest_info::LatestInfo,
    },
};

pub(crate) struct CacheMap {
    pub(crate) caches: DashMap<u64, GroupOfFramesMap>,
    // short term lock for latest info
    pub(crate) latest_info: std::sync::RwLock<LatestInfo>,
}

impl CacheMap {
    pub(crate) fn new() -> Self {
        Self {
            caches: DashMap::new(),
            latest_info: std::sync::RwLock::new(LatestInfo::new()),
        }
    }
}

#[async_trait::async_trait]
impl Cache for CacheMap {
    fn get_latest_group_id(&self) -> u64 {
        self.latest_info.read().unwrap().group_id
    }

    fn get_latest_receiver(&self) -> tokio::sync::watch::Receiver<Arc<DataObject>> {
        self.latest_info.read().unwrap().get_receiver()
    }

    async fn set_latest_object(&self, object: DataObject) {
        let group_id = match object.group_id() {
            Some(id) => id,
            None => self.get_latest_group_id(),
        };
        let cache = self
            .caches
            .entry(group_id)
            .or_insert_with(GroupOfFramesMap::new);
        let object = Arc::new(object);
        cache.frames.write().await.push(object.clone());
        let mut latest_info = self.latest_info.write().unwrap();
        latest_info.group_id = group_id;
        latest_info.set_latest_object(object);
    }

    async fn get_group(&self, group_id: u64) -> Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>> {
        self.caches.get(&group_id).unwrap().value().frames.clone()
    }
}
