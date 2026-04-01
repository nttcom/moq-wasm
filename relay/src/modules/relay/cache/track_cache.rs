use std::{collections::BTreeMap, sync::Arc};

use tokio::sync::RwLock;

use crate::modules::{
    core::data_object::DataObject,
    relay::{cache::group_cache::GroupCache, types::CacheLocation},
};

pub(crate) struct TrackCache {
    groups: RwLock<BTreeMap<u64, Arc<GroupCache>>>,
    latest: RwLock<Option<CacheLocation>>,
}

impl TrackCache {
    pub(crate) fn new() -> Self {
        Self {
            groups: RwLock::new(BTreeMap::new()),
            latest: RwLock::new(None),
        }
    }

    async fn ensure_group(&self, group_id: u64) -> Arc<GroupCache> {
        if let Some(existing) = self.groups.read().await.get(&group_id).cloned() {
            return existing;
        }
        let mut groups = self.groups.write().await;
        groups
            .entry(group_id)
            .or_insert_with(|| Arc::new(GroupCache::new()))
            .clone()
    }

    pub(crate) async fn append_object(&self, group_id: u64, object: DataObject) -> u64 {
        let group = self.ensure_group(group_id).await;
        let object = Arc::new(object);
        let index = group.append(object).await;
        *self.latest.write().await = Some(CacheLocation { group_id, index });
        index
    }

    pub(crate) async fn close_group(&self, group_id: u64) {
        let group = self.ensure_group(group_id).await;
        group.mark_end_of_group().await;
    }

    pub(crate) async fn latest_location(&self) -> Option<CacheLocation> {
        self.latest.read().await.clone()
    }

    pub(crate) async fn latest_group_id(&self) -> Option<u64> {
        self.groups.read().await.keys().next_back().copied()
    }

    pub(crate) async fn get_object(&self, group_id: u64, index: u64) -> Option<Arc<DataObject>> {
        let group = self.groups.read().await.get(&group_id).cloned()?;
        group.get(index).await
    }

    pub(crate) async fn is_group_closed(&self, group_id: u64) -> bool {
        let group = self.groups.read().await.get(&group_id).cloned();
        match group {
            Some(g) => g.is_closed().await,
            None => false,
        }
    }
}
