use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::{
    core::data_object::DataObject,
    enums::Location,
    relaies::caches::{
        cache::Cache, group_of_frames_map::GroupOfFramesMap, latest_info::LatestInfo,
    },
};

pub(crate) struct CacheMap {
    pub(crate) caches: DashMap<u64, GroupOfFramesMap>,
    // short term lock for latest info
    latest_object_notifier: tokio::sync::broadcast::Sender<LatestInfo>,
}

impl CacheMap {
    pub(crate) fn new() -> Self {
        let (sender, _receiver) = tokio::sync::broadcast::channel(128);
        Self {
            caches: DashMap::new(),
            latest_object_notifier: sender,
        }
    }
}

#[async_trait::async_trait]
impl Cache for CacheMap {
    fn get_latest_group_id(&self) -> u64 {
        self.caches
            .iter()
            .map(|entry| *entry.key())
            .max()
            .unwrap_or(0)
    }

    fn get_latest_receiver(&self) -> tokio::sync::broadcast::Receiver<LatestInfo> {
        self.latest_object_notifier.subscribe()
    }

    async fn set_latest_object(&self, track_key: u128, group_id: u64, object: DataObject) {
        let cache = self
            .caches
            .entry(group_id)
            .or_insert_with(GroupOfFramesMap::new);
        let object = Arc::new(object);
        cache.frames.write().await.push(object.clone());
        let _ = self.latest_object_notifier.send(LatestInfo::LatestObject {
            track_key,
            group_id,
            offset: cache.frames.read().await.len() as u64 - 1,
        });
    }

    async fn notify_end_of_group(&self, track_key: u128, group_id: u64) {
        let _ = self.latest_object_notifier.send(LatestInfo::EndOfGroup {
            track_key,
            group_id,
        });
    }

    async fn get_group(&self, group_id: u64) -> Arc<tokio::sync::RwLock<Vec<Arc<DataObject>>>> {
        self.caches.get(&group_id).unwrap().value().frames.clone()
    }

    async fn get_offset(&self, location: Location) -> Option<u64> {
        let group_id = location.group_id;
        let object_id = location.object_id;
        let cache = self.caches.get(&group_id)?;
        let frames = cache.frames.read().await;
        frames
            .iter()
            .position(|object| object.object_id() == Some(object_id))
            .map(|index| index as u64)
    }
}
