use std::sync::Arc;

use crate::modules::relay::{
    cache::store::TrackCacheStore,
    notifications::subgroup_opened_notifier_map::SubgroupOpenedNotifierMap,
};

pub(crate) struct RelayStore {
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) subgroup_opened_notifier_map: Arc<SubgroupOpenedNotifierMap>,
}

impl RelayStore {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cache_store: Arc::new(TrackCacheStore::new()),
            subgroup_opened_notifier_map: Arc::new(SubgroupOpenedNotifierMap::new()),
        })
    }
}
