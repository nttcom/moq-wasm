use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::relaies::caches::cache::Cache;

pub(crate) struct RelayProperties {
    pub(crate) object_queue: DashMap<u64, Arc<dyn Cache>>,
    pub(crate) joinset: tokio::task::JoinSet<()>,
}

impl RelayProperties {
    pub(crate) fn new() -> Self {
        Self {
            object_queue: DashMap::new(),
            joinset: tokio::task::JoinSet::new(),
        }
    }
}
