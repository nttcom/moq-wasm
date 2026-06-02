use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::{relay::cache::track_cache::TrackCache, types::TrackKey};

pub(crate) struct TrackCacheStore {
    caches: DashMap<TrackKey, Arc<TrackCache>>,
}

impl TrackCacheStore {
    pub(crate) fn new() -> Self {
        Self {
            caches: DashMap::new(),
        }
    }

    pub(crate) fn get(&self, track_key: TrackKey) -> Option<Arc<TrackCache>> {
        // clone the Arc to drop the Ref and release the DashMap shard lock
        self.caches.get(&track_key).map(|v| v.clone())
    }

    pub(crate) fn get_or_create(&self, track_key: TrackKey) -> Arc<TrackCache> {
        self.caches
            .entry(track_key)
            .or_insert_with(|| Arc::new(TrackCache::new()))
            .clone()
    }
}
