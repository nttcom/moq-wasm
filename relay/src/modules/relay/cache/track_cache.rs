use std::{collections::BTreeMap, sync::Arc};

use tokio::sync::RwLock;

use crate::modules::{
    core::data_object::DataObject,
    relay::{cache::group_cache::GroupCache, types::StreamSubgroupId},
};

pub(crate) struct TrackCache {
    stream_groups: RwLock<BTreeMap<u64, BTreeMap<StreamSubgroupId, Arc<GroupCache>>>>,
    datagram_groups: RwLock<BTreeMap<u64, Arc<GroupCache>>>,
}

impl TrackCache {
    pub(crate) fn new() -> Self {
        Self {
            stream_groups: RwLock::new(BTreeMap::new()),
            datagram_groups: RwLock::new(BTreeMap::new()),
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
        object_id: Option<u64>,
        object: DataObject,
    ) {
        let group = self.ensure_stream_subgroup(group_id, subgroup_id).await;
        group.append(object_id, Arc::new(object)).await;
    }

    pub(crate) async fn append_datagram_object(
        &self,
        group_id: u64,
        object_id: Option<u64>,
        object: DataObject,
    ) {
        // Datagrams have no subgroup header, so a resolved object_id is always expected.
        let Some(object_id) = object_id else {
            tracing::error!(group_id, "unexpected: datagram object without object_id");
            return;
        };
        let group = self.ensure_datagram_group(group_id).await;
        group.append(Some(object_id), Arc::new(object)).await;
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

    /// Returns the Largest Location as defined in the MoQT spec.
    pub(crate) async fn largest_location(&self) -> Option<moqt::Location> {
        let (group_id, cache) = {
            let groups = self.stream_groups.read().await;
            let (&group_id, subgroups) = groups.iter().next_back()?;
            let (_, cache) = subgroups.iter().next_back()?;
            (group_id, cache.clone())
        };

        Some(moqt::Location {
            group_id,
            object_id: cache.largest_object_id().await?,
        })
    }

    pub(crate) async fn get_stream_header_or_wait(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
    ) -> Option<Arc<DataObject>> {
        let group = self
            .stream_groups
            .read()
            .await
            .get(&group_id)
            .and_then(|subgroups| subgroups.get(subgroup_id))
            .cloned()?;
        group.header_or_wait().await
    }

    pub(crate) async fn stream_object_from_or_wait(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        object_id: u64,
    ) -> Option<(u64, Arc<DataObject>)> {
        let group = self
            .stream_groups
            .read()
            .await
            .get(&group_id)
            .and_then(|subgroups| subgroups.get(subgroup_id))
            .cloned()?;
        group.object_from_or_wait(object_id).await
    }

    pub(crate) async fn datagram_object_from_or_wait(
        &self,
        group_id: u64,
        object_id: u64,
    ) -> Option<(u64, Arc<DataObject>)> {
        let group = self.datagram_groups.read().await.get(&group_id).cloned()?;
        group.object_from_or_wait(object_id).await
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
            let Some(header) = cache.header().await else {
                tracing::error!("unexpected: GroupCache has no subgroup header");
                continue;
            };
            let (publisher_priority, subgroup_id) = match header.as_ref() {
                DataObject::SubgroupHeader(h) => (h.publisher_priority, h.subgroup_id.resolve()),
                _ => {
                    tracing::error!("unexpected: GroupCache header is not a SubgroupHeader");
                    continue;
                }
            };

            for (current_object_id, object) in cache.objects_snapshot().await {
                let field = match object.as_ref() {
                    DataObject::SubgroupObject(f) => f,
                    _ => continue,
                };

                // Apply range filter for first and last groups
                if group_id == start.group_id && current_object_id < start.object_id {
                    continue;
                }
                // End Location is exclusive; `>=` excludes the object at end.object_id.
                // end.object_id == 0 is a special case meaning "the entire group".
                if group_id == end.group_id
                    && end.object_id != 0
                    && current_object_id >= end.object_id
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
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        // Act / Assert: a header is not an object, so None is returned
        assert!(cache.largest_location().await.is_none());
    }

    #[tokio::test]
    async fn largest_location_returns_single_object() {
        // Arrange: one object at id 0 in group 0
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0]).await;
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: the only object's location is returned
        assert_eq!(loc.group_id, 0);
        assert_eq!(loc.object_id, 0);
    }

    #[tokio::test]
    async fn largest_location_returns_max_object_id() {
        // Arrange: sequential object_ids 0, 1, 2 in group 0
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0, 1, 2]).await;
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: the largest object_id is returned
        assert_eq!(loc.group_id, 0);
        assert_eq!(loc.object_id, 2);
    }

    #[tokio::test]
    async fn largest_location_returns_max_id_across_gap() {
        // Arrange: gapped object_ids 5, 8 in group 0
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[5, 8]).await;
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: the largest object_id is returned even with a gap
        assert_eq!(loc.group_id, 0);
        assert_eq!(loc.object_id, 8);
    }

    #[tokio::test]
    async fn largest_location_returns_latest_group() {
        // Arrange: one object at id 0 in each of group 0 and group 1
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0]).await;
        fill_group_with_ids(&cache, 1, &[0]).await;
        // Act
        let loc = cache.largest_location().await.unwrap();
        // Assert: the latest group's location is returned
        assert_eq!(loc.group_id, 1);
        assert_eq!(loc.object_id, 0);
    }

    async fn fill_group(cache: &TrackCache, group_id: u64, count: u64) {
        // object_ids 0, 1, ..., count-1
        let object_ids: Vec<u64> = (0..count).collect();
        fill_group_with_ids(cache, group_id, &object_ids).await;
    }

    // Fills a group with objects at the given (non-sequential) absolute object_ids.
    async fn fill_group_with_ids(cache: &TrackCache, group_id: u64, object_ids: &[u64]) {
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(group_id, &subgroup, None, make_header(group_id))
            .await;
        for &id in object_ids {
            cache
                .append_stream_object(group_id, &subgroup, Some(id), make_object(0))
                .await;
        }
    }

    fn object_ids(objects: &[moqt::FetchObjectField]) -> Vec<(u64, u64)> {
        objects.iter().map(|o| (o.group_id, o.object_id)).collect()
    }

    #[tokio::test]
    async fn get_fetch_objects_excludes_end_object() {
        // Arrange: group 0 with object_ids 0..=4
        let cache = TrackCache::new();
        fill_group(&cache, 0, 5).await;
        // Act: fetch [{0,0}, {0,3}) -> end object 3 is excluded
        let objects = cache
            .get_fetch_objects(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 3,
                },
            )
            .await;
        // Assert: 0,1,2 included; 3 (the End) excluded
        assert_eq!(object_ids(&objects), vec![(0, 0), (0, 1), (0, 2)]);
    }

    #[tokio::test]
    async fn get_fetch_objects_end_object_zero_returns_entire_group() {
        // Arrange: group 0 with object_ids 0..=4
        let cache = TrackCache::new();
        fill_group(&cache, 0, 5).await;
        // Act: end.object_id == 0 means the entire end group
        let objects = cache
            .get_fetch_objects(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
            )
            .await;
        // Assert: all objects are included
        assert_eq!(
            object_ids(&objects),
            vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4)]
        );
    }

    #[tokio::test]
    async fn get_fetch_objects_filters_range_with_gaps() {
        // Arrange: group 0 with gapped object_ids 0, 3, 5, 8
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0, 3, 5, 8]).await;
        // Act: fetch [{0,3}, {0,8}) -> start 3 inclusive, end 8 exclusive
        let objects = cache
            .get_fetch_objects(
                moqt::Location {
                    group_id: 0,
                    object_id: 3,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 8,
                },
            )
            .await;
        // Assert: 0 skipped (< start), 8 excluded (>= end); 3 and 5 survive across the gaps
        assert_eq!(object_ids(&objects), vec![(0, 3), (0, 5)]);
    }

    #[tokio::test]
    async fn get_fetch_objects_spans_groups_with_exclusive_end() {
        // Arrange: group 0 and group 1, each with object_ids 0..=4
        let cache = TrackCache::new();
        fill_group(&cache, 0, 5).await;
        fill_group(&cache, 1, 5).await;
        // Act: fetch [{0,2}, {1,3}) -> start object 2 (inclusive), end object 3 (exclusive)
        let objects = cache
            .get_fetch_objects(
                moqt::Location {
                    group_id: 0,
                    object_id: 2,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 3,
                },
            )
            .await;
        // Assert: g0 from object 2; g1 up to (not including) object 3
        assert_eq!(
            object_ids(&objects),
            vec![(0, 2), (0, 3), (0, 4), (1, 0), (1, 1), (1, 2)]
        );
    }
}
