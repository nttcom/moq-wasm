use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use tokio::sync::{RwLock, broadcast};

use crate::modules::{
    core::data_object::DataObject,
    enums::{GroupOrder, Location},
    relay::{
        cache::{group_cache::GroupCache, latest_event::LatestCacheEvent},
        types::CacheLocation,
    },
    types::TrackKey,
};

pub(crate) struct TrackCache {
    groups: RwLock<BTreeMap<u64, Arc<GroupCache>>>,
    latest: RwLock<Option<CacheLocation>>,
    closed_groups: RwLock<BTreeSet<u64>>,
    stream_headers: RwLock<BTreeMap<u64, Arc<DataObject>>>,
    latest_notifier: broadcast::Sender<LatestCacheEvent>,
}

impl TrackCache {
    pub(crate) fn new() -> Self {
        let latest_notifier = broadcast::channel(512).0;
        Self {
            groups: RwLock::new(BTreeMap::new()),
            latest: RwLock::new(None),
            closed_groups: RwLock::new(BTreeSet::new()),
            stream_headers: RwLock::new(BTreeMap::new()),
            latest_notifier,
        }
    }

    pub(crate) fn subscribe_latest(&self) -> broadcast::Receiver<LatestCacheEvent> {
        self.latest_notifier.subscribe()
    }

    pub(crate) async fn ensure_group(&self, group_id: u64) -> Arc<GroupCache> {
        if let Some(existing) = self.groups.read().await.get(&group_id).cloned() {
            return existing;
        }

        let mut groups = self.groups.write().await;
        groups
            .entry(group_id)
            .or_insert_with(|| Arc::new(GroupCache::new()))
            .clone()
    }

    pub(crate) async fn append_object(
        &self,
        track_key: TrackKey,
        group_id: u64,
        object: DataObject,
    ) -> u64 {
        let group = self.ensure_group(group_id).await;
        let object = Arc::new(object);

        if matches!(object.as_ref(), DataObject::SubgroupHeader(_)) {
            self.stream_headers
                .write()
                .await
                .insert(group_id, object.clone());
        }

        let index = group.append(object).await;
        *self.latest.write().await = Some(CacheLocation { group_id, index });

        let _ = self.latest_notifier.send(LatestCacheEvent::ObjectAppended {
            track_key,
            group_id,
            index,
        });
        index
    }

    pub(crate) async fn close_group(&self, track_key: TrackKey, group_id: u64) {
        let group = self.ensure_group(group_id).await;
        group.mark_end_of_group().await;
        self.closed_groups.write().await.insert(group_id);
        let _ = self.latest_notifier.send(LatestCacheEvent::GroupClosed {
            track_key,
            group_id,
        });
    }

    pub(crate) async fn latest_location(&self) -> Option<CacheLocation> {
        self.latest.read().await.clone()
    }

    pub(crate) async fn latest_group_id(&self) -> Option<u64> {
        self.groups.read().await.keys().next_back().copied()
    }

    pub(crate) async fn group_header(&self, group_id: u64) -> Option<Arc<DataObject>> {
        self.stream_headers.read().await.get(&group_id).cloned()
    }

    pub(crate) async fn get_object(&self, group_id: u64, index: u64) -> Option<Arc<DataObject>> {
        let group = self.groups.read().await.get(&group_id).cloned();
        match group {
            Some(group) => group.get(index).await,
            None => None,
        }
    }

    pub(crate) async fn is_group_closed(&self, group_id: u64) -> bool {
        if self.closed_groups.read().await.contains(&group_id) {
            return true;
        }
        let group = self.groups.read().await.get(&group_id).cloned();
        match group {
            Some(group) => group.is_closed().await,
            None => false,
        }
    }

    pub(crate) async fn resolve_index(&self, location: &Location) -> Option<u64> {
        let group = self.groups.read().await.get(&location.group_id).cloned();
        match group {
            Some(group) => group.index_of_object_id(location.object_id).await,
            None => None,
        }
    }

    pub(crate) async fn next_group_id(
        &self,
        current_group_id: u64,
        group_order: &GroupOrder,
    ) -> Option<u64> {
        let groups = self.groups.read().await;
        match group_order {
            GroupOrder::Descending => groups
                .keys()
                .rev()
                .find(|group_id| **group_id < current_group_id)
                .copied(),
            GroupOrder::Ascending | GroupOrder::Publisher => groups
                .keys()
                .find(|group_id| **group_id > current_group_id)
                .copied(),
        }
    }
}
