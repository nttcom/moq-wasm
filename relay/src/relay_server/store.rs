use std::sync::Arc;

use crate::modules::relay::{
    cache::store::TrackCacheStore, notifications::track_notifier::ObjectNotifyProducerMap,
};

pub(crate) struct RelayStore {
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) object_notify_producer_map: Arc<ObjectNotifyProducerMap>,
}

impl RelayStore {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cache_store: Arc::new(TrackCacheStore::new()),
            object_notify_producer_map: Arc::new(ObjectNotifyProducerMap::new()),
        })
    }
}
