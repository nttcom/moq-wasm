use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::relaies::caches::cache::Cache;

pub(crate) struct CacheRepository {
    pub(crate) object_queue: DashMap<u128, Arc<dyn Cache>>,
}

impl CacheRepository {
    pub(crate) fn new() -> Self {
        CacheRepository {
            object_queue: DashMap::new(),
        }
    }

    pub(crate) fn insert_cache(&self, id: u128, cache: Arc<dyn Cache>) {
        self.object_queue.insert(id, cache);
    }

    pub(crate) fn get_cache(&self, id: &u128) -> Arc<dyn Cache> {
        self.object_queue.get(id).unwrap().value().clone()
    }
}
