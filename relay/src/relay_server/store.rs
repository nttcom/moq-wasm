use std::sync::Arc;

use crate::modules::relay::cache::store::TrackCacheStore;

pub(crate) struct RelayStore {
    pub(crate) cache_store: Arc<TrackCacheStore>,
}

impl RelayStore {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cache_store: Arc::new(TrackCacheStore::new()),
        })
    }
}
