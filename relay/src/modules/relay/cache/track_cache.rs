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

    /// Returns the cache position of the most recently appended object across all groups and subgroups.
    ///
    /// FIXME: When multiple subgroups are written concurrently, the ordering of writes to
    /// `self.latest` is not guaranteed. The returned position may not correspond to the
    /// MoQT-largest object if subgroups interleave. This is a known limitation to be
    /// addressed in a cache redesign task.
    pub(crate) async fn latest_cache_location(&self) -> Option<CacheLocation> {
        self.latest.read().await.clone()
    }

    /// Returns the Largest Location as defined in the MoQT spec.
    ///
    /// NOTE: This scans the latest subgroup to resolve object_id from the delta chain,
    /// which is O(n) in the number of objects in the subgroup.
    /// Ideally, the cache should track {group_id, object_id} directly at ingestion time
    /// to make this O(1). Deferred to a cache redesign task.
    pub(crate) async fn largest_location(&self) -> Option<moqt::Location> {
        let groups = self.stream_groups.read().await;
        let (&group_id, subgroups) = groups.iter().next_back()?;
        let (_, cache) = subgroups.iter().next_back()?;
        let cache = cache.clone();
        drop(groups);

        let objects = cache.snapshot().await;
        let mut prev_object_id: Option<u64> = None;
        let mut last_object_id: Option<u64> = None;

        for obj in objects.iter().skip(1) {
            let DataObject::SubgroupObject(field) = obj.as_ref() else {
                continue;
            };
            let current_object_id = field.resolve_object_id(prev_object_id);
            prev_object_id = Some(current_object_id);
            last_object_id = Some(current_object_id);
        }

        Some(moqt::Location {
            group_id,
            object_id: last_object_id?,
        })
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

    pub(crate) async fn get_fetch_objects(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) -> Vec<moqt::FetchObjectField> {
        // Collect all GroupCaches in the requested group range, tagged with their group_id.
        // Example for start={group=1, object=3}, end={group=2, object=5}:
        //   [(1, GroupCache),  // group=1, subgroup=0
        //    (1, GroupCache),  // group=1, subgroup=1  <- group 1 has 2 subgroups
        //    (2, GroupCache)]  // group=2, subgroup=0
        let caches_in_range: Vec<(u64, Arc<GroupCache>)> = {
            // NOTE: Datagram objects are not included.
            let groups = self.stream_groups.read().await;
            let mut caches_in_range = Vec::new();
            for (&group_id, subgroup_map) in groups.range(start.group_id..=end.group_id) {
                for cache in subgroup_map.values() {
                    caches_in_range.push((group_id, cache.clone()));
                }
            }
            caches_in_range
        };

        // Convert each GroupCache into FetchObjectFields.
        // For the first and last group, filter objects by object_id.
        // Example for start={group=1, object=3}, end={group=2, object=5}:
        //   group=1: skip object_id 0,1,2 -> include 3,4,...
        //   group=2: include object_id 0,1,2,3,4 -> stop before 5 (exclusive)
        let mut fetch_objects = Vec::new();
        for (group_id, cache) in caches_in_range {
            let subgroup_objects = cache.snapshot().await;

            let Some(header) = subgroup_objects.first() else {
                tracing::error!("unexpected: GroupCache is empty");
                continue;
            };
            let (publisher_priority, subgroup_id) = match header.as_ref() {
                DataObject::SubgroupHeader(h) => (h.publisher_priority, h.subgroup_id.resolve()),
                _ => {
                    tracing::error!("unexpected: GroupCache does not start with SubgroupHeader");
                    continue;
                }
            };

            let mut prev_object_id: Option<u64> = None;
            // skip index=0 which is SubgroupHeader
            for obj in subgroup_objects.iter().skip(1) {
                let field = match obj.as_ref() {
                    DataObject::SubgroupObject(f) => f,
                    _ => continue,
                };
                // Resolve absolute object_id from object_id_delta
                let current_object_id = field.resolve_object_id(prev_object_id);
                prev_object_id = Some(current_object_id);

                // Apply range filter for first and last groups
                if group_id == start.group_id && current_object_id < start.object_id {
                    continue;
                }
                // end.object_id == 0 means entire group is requested
                if group_id == end.group_id
                    && end.object_id != 0
                    && current_object_id > end.object_id
                {
                    break;
                }

                // Convert to FetchObjectField and append to fetch_objects
                let fetch_object = match &field.subgroup_object {
                    moqt::SubgroupObject::Payload { data, .. } => {
                        moqt::FetchObject::Payload(data.clone())
                    }
                    moqt::SubgroupObject::Status { code, .. } => {
                        let Ok(status) = moqt::ObjectStatus::try_from(*code as u8) else {
                            continue;
                        };
                        moqt::FetchObject::Status(status)
                    }
                };
                fetch_objects.push(moqt::FetchObjectField::new(
                    group_id,
                    subgroup_id,
                    current_object_id,
                    publisher_priority,
                    field.extension_headers.clone(),
                    fetch_object,
                ));
            }
        }
        fetch_objects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};

    fn make_header(group_id: u64) -> DataObject {
        DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            group_id,
            SubgroupId::Value(0),
            0,
            false,
            false,
        ))
    }

    fn make_object(delta: u64) -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: delta,
            extension_headers: ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            subgroup_object: SubgroupObject::new_payload(Bytes::from(vec![])),
        })
    }

    #[tokio::test]
    async fn largest_location_returns_none_when_empty() {
        // Arrange: empty cache
        let cache = TrackCache::new();
        // Act / Assert: no objects exist, so None is returned
        assert!(cache.largest_location().await.is_none());
    }

    #[tokio::test]
    async fn largest_location_returns_none_when_only_header() {
        // Arrange: append a SubgroupHeader only, no SubgroupObject
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, make_header(0))
            .await;
        // Act / Assert: a header is not an object, so None is returned
        assert!(cache.largest_location().await.is_none());
    }

    #[tokio::test]
    async fn largest_location_returns_single_object() {
        // Arrange: one SubgroupObject with delta=0 in group_id=0
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, make_header(0))
            .await;
        cache
            .append_stream_object(0, &subgroup, make_object(0))
            .await;
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: object_id resolves to 0 (delta=0, no previous)
        assert_eq!(loc.group_id, 0);
        assert_eq!(loc.object_id, 0);
    }

    #[tokio::test]
    async fn largest_location_resolves_sequential_delta_chain() {
        // Arrange: three objects with delta=0, yielding object_ids 0, 1, 2
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, make_header(0))
            .await;
        cache
            .append_stream_object(0, &subgroup, make_object(0))
            .await; // object_id = 0
        cache
            .append_stream_object(0, &subgroup, make_object(0))
            .await; // object_id = 1
        cache
            .append_stream_object(0, &subgroup, make_object(0))
            .await; // object_id = 2
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: largest object_id in the delta chain is returned
        assert_eq!(loc.group_id, 0);
        assert_eq!(loc.object_id, 2);
    }

    #[tokio::test]
    async fn largest_location_resolves_delta_with_gap() {
        // Arrange: first object has delta=5 (object_id=5), second has delta=2 (object_id=8)
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, make_header(0))
            .await;
        cache
            .append_stream_object(0, &subgroup, make_object(5))
            .await; // object_id = 5
        cache
            .append_stream_object(0, &subgroup, make_object(2))
            .await; // object_id = 5+1+2 = 8
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: delta gaps are resolved correctly
        assert_eq!(loc.group_id, 0);
        assert_eq!(loc.object_id, 8);
    }

    #[tokio::test]
    async fn largest_location_returns_latest_group() {
        // Arrange: objects in group 0 and group 1
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, make_header(0))
            .await;
        cache
            .append_stream_object(0, &subgroup, make_object(0))
            .await; // group=0, object_id=0
        cache
            .append_stream_object(1, &subgroup, make_header(1))
            .await;
        cache
            .append_stream_object(1, &subgroup, make_object(0))
            .await; // group=1, object_id=0
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: the latest group's location is returned
        assert_eq!(loc.group_id, 1);
        assert_eq!(loc.object_id, 0);
    }
}
