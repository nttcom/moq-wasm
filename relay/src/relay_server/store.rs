use std::sync::Arc;

use crate::modules::relay::{cache::store::TrackCacheStore, notifications::sender_map::SenderMap};

pub(crate) struct RelayStore {
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) sender_map: Arc<SenderMap>,
}

impl RelayStore {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cache_store: Arc::new(TrackCacheStore::new()),
            sender_map: Arc::new(SenderMap::new()),
        })
    }
}
