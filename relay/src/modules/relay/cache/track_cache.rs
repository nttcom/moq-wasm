use std::{collections::BTreeMap, sync::Arc};

use tokio::sync::RwLock;

use crate::modules::{
    core::data_object::DataObject,
    relay::{
        cache::group_cache::GroupCache,
        types::{CacheLocation, StreamSubgroupId},
    },
};

pub(crate) struct TrackCache {
    stream_groups: RwLock<BTreeMap<u64, BTreeMap<StreamSubgroupId, Arc<GroupCache>>>>,
    datagram_groups: RwLock<BTreeMap<u64, Arc<GroupCache>>>,
    latest: RwLock<Option<CacheLocation>>,
}

impl TrackCache {
    pub(crate) fn new() -> Self {
        Self {
            stream_groups: RwLock::new(BTreeMap::new()),
            datagram_groups: RwLock::new(BTreeMap::new()),
            latest: RwLock::new(None),
        }
    }

    async fn ensure_stream_subgroup(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
    ) -> Arc<GroupCache> {
        if let Some(existing) = self
            .stream_groups
            .read()
            .await
            .get(&group_id)
            .and_then(|subgroups| subgroups.get(subgroup_id))
            .cloned()
        {
            return existing;
        }

        let mut groups = self.stream_groups.write().await;
        groups
            .entry(group_id)
            .or_default()
            .entry(subgroup_id.clone())
            .or_insert_with(|| Arc::new(GroupCache::new()))
            .clone()
    }

    async fn ensure_datagram_group(&self, group_id: u64) -> Arc<GroupCache> {
        if let Some(existing) = self.datagram_groups.read().await.get(&group_id).cloned() {
            return existing;
        }

        let mut groups = self.datagram_groups.write().await;
        groups
            .entry(group_id)
            .or_insert_with(|| Arc::new(GroupCache::new()))
            .clone()
    }

    pub(crate) async fn append_stream_object(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        object: DataObject,
    ) -> u64 {
        let group = self.ensure_stream_subgroup(group_id, subgroup_id).await;
        let object = Arc::new(object);
        let index = group.append(object).await;
        *self.latest.write().await = Some(CacheLocation::Stream {
            group_id,
            subgroup_id: subgroup_id.clone(),
            index,
        });
        index
    }

    pub(crate) async fn append_datagram_object(&self, group_id: u64, object: DataObject) -> u64 {
        let group = self.ensure_datagram_group(group_id).await;
        let object = Arc::new(object);
        let index = group.append(object).await;
        *self.latest.write().await = Some(CacheLocation::Datagram { group_id, index });
        index
    }

    pub(crate) async fn close_stream_subgroup(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
    ) {
        let group = self.ensure_stream_subgroup(group_id, subgroup_id).await;
        group.mark_end_of_group().await;
    }

    pub(crate) async fn close_datagram_group(&self, group_id: u64) {
        let group = self.ensure_datagram_group(group_id).await;
        group.mark_end_of_group().await;
    }

    pub(crate) async fn latest_location(&self) -> Option<CacheLocation> {
        self.latest.read().await.clone()
    }

    pub(crate) async fn latest_group_id(&self) -> Option<u64> {
        let stream_group = self.stream_groups.read().await.keys().next_back().copied();
        let datagram_group = self
            .datagram_groups
            .read()
            .await
            .keys()
            .next_back()
            .copied();
        stream_group.max(datagram_group)
    }

    pub(crate) async fn get_stream_object_or_wait(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        index: u64,
    ) -> Option<Arc<DataObject>> {
        let group = self
            .stream_groups
            .read()
            .await
            .get(&group_id)
            .and_then(|subgroups| subgroups.get(subgroup_id))
            .cloned()?;
        group.get_or_wait(index).await
    }

    pub(crate) async fn get_datagram_object_or_wait(
        &self,
        group_id: u64,
        index: u64,
    ) -> Option<Arc<DataObject>> {
        let group = self.datagram_groups.read().await.get(&group_id).cloned()?;
        group.get_or_wait(index).await
    }

    pub(crate) async fn has_stream_group(&self, group_id: u64) -> bool {
        self.stream_groups.read().await.contains_key(&group_id)
    }

    pub(crate) async fn has_datagram_group(&self, group_id: u64) -> bool {
        self.datagram_groups.read().await.contains_key(&group_id)
    }

    pub(crate) async fn stream_subgroups(&self, group_id: u64) -> Vec<StreamSubgroupId> {
        self.stream_groups
            .read()
            .await
            .get(&group_id)
            .map(|subgroups| subgroups.keys().cloned().collect())
            .unwrap_or_default()
    }
}
