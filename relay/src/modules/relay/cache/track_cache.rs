use std::{
    cmp::Ordering,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
    time::Duration,
};

use tokio::sync::RwLock;

use crate::modules::{
    core::data_object::DataObject,
    relay::{
        cache::{group_cache::GroupCache, known_ranges::KnownRanges},
        types::StreamSubgroupId,
    },
};

pub(crate) struct TrackCache {
    stream_groups: RwLock<BTreeMap<u64, BTreeMap<StreamSubgroupId, Arc<GroupCache>>>>,
    datagram_groups: RwLock<BTreeMap<u64, Arc<GroupCache>>>,
    known_ranges: RwLock<KnownRanges>,
    live_ingest_count: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FetchRangeResolution {
    Serve { end_location: moqt::Location },
    InvalidRange,
    NoObjects,
    NotCovered,
}

impl TrackCache {
    pub(crate) fn new() -> Self {
        Self {
            stream_groups: RwLock::new(BTreeMap::new()),
            datagram_groups: RwLock::new(BTreeMap::new()),
            known_ranges: RwLock::new(KnownRanges::default()),
            live_ingest_count: AtomicUsize::new(0),
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

    pub(crate) async fn append_live_stream_object(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        object_id: Option<u64>,
        object: DataObject,
    ) {
        self.append_stream_object(group_id, subgroup_id, object_id, object)
            .await;
        if let Some(object_id) = object_id {
            self.known_ranges.write().await.insert(
                moqt::Location {
                    group_id,
                    object_id: 0,
                },
                moqt::Location {
                    group_id,
                    object_id: object_id.saturating_add(1),
                },
            );
        }
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
        let caches = self.stream_group_caches(group_id).await;
        if Self::all_subgroups_closed_for_group(&caches).await {
            // This preserves the previous optimistic live coverage model: once all
            // currently known subgroups close, no later subgroup for the group is assumed.
            self.known_ranges.write().await.insert(
                moqt::Location {
                    group_id,
                    object_id: 0,
                },
                moqt::Location {
                    group_id,
                    object_id: 0,
                },
            );
        }
    }

    pub(crate) async fn close_datagram_group(&self, group_id: u64) {
        let group = self.ensure_datagram_group(group_id).await;
        group.mark_end_of_group().await;
    }

    pub(crate) async fn evict(&self, ttl: Duration) {
        // Snapshot group handles so per-group eviction runs without holding the map lock.
        let stream_groups: Vec<(u64, StreamSubgroupId, Arc<GroupCache>)> = {
            let groups = self.stream_groups.read().await;
            groups
                .iter()
                .flat_map(|(&group_id, subgroups)| {
                    subgroups.iter().map(move |(subgroup_id, group)| {
                        (group_id, subgroup_id.clone(), group.clone())
                    })
                })
                .collect()
        };
        let mut removable_streams: Vec<(u64, StreamSubgroupId)> = Vec::new();
        for (group_id, subgroup_id, group) in stream_groups {
            if let Some(evicted_objects) = group.evict_expired_objects(ttl).await {
                // Coverage is track-level, while eviction reports object IDs per subgroup.
                // Removing this range may under-claim if another subgroup still has fresh
                // objects with the same IDs, but it never over-claims deleted knowledge.
                self.known_ranges.write().await.remove_range(
                    moqt::Location {
                        group_id,
                        object_id: evicted_objects.start,
                    },
                    moqt::Location {
                        group_id,
                        object_id: evicted_objects.end,
                    },
                );
            }
            if group.is_evictable().await {
                removable_streams.push((group_id, subgroup_id));
            }
        }
        if !removable_streams.is_empty() {
            let mut groups = self.stream_groups.write().await;
            for (group_id, subgroup_id) in removable_streams {
                if let Some(subgroups) = groups.get_mut(&group_id) {
                    subgroups.remove(&subgroup_id);
                    if subgroups.is_empty() {
                        groups.remove(&group_id);
                    }
                }
            }
        }

        let datagram_groups: Vec<(u64, Arc<GroupCache>)> = {
            let groups = self.datagram_groups.read().await;
            groups
                .iter()
                .map(|(&group_id, group)| (group_id, group.clone()))
                .collect()
        };
        let mut removable_datagrams: Vec<u64> = Vec::new();
        for (group_id, group) in datagram_groups {
            let _ = group.evict_expired_objects(ttl).await;
            if group.is_evictable().await {
                removable_datagrams.push(group_id);
            }
        }
        if !removable_datagrams.is_empty() {
            let mut groups = self.datagram_groups.write().await;
            for group_id in removable_datagrams {
                groups.remove(&group_id);
            }
        }
    }

    /// Returns the Largest Location as defined in the MoQT spec.
    pub(crate) async fn largest_location(&self) -> Option<moqt::Location> {
        let groups = {
            let groups = self.stream_groups.read().await;
            groups
                .iter()
                .rev()
                .map(|(&group_id, subgroups)| {
                    (group_id, subgroups.values().cloned().collect::<Vec<_>>())
                })
                .collect::<Vec<_>>()
        };

        for (group_id, caches) in groups {
            let mut largest_object_id = None;
            for cache in caches {
                if let Some(object_id) = cache.largest_object_id().await {
                    largest_object_id = Some(
                        largest_object_id.map_or(object_id, |current: u64| current.max(object_id)),
                    );
                }
            }
            if let Some(object_id) = largest_object_id {
                return Some(moqt::Location {
                    group_id,
                    object_id,
                });
            }
        }

        None
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

    pub(crate) async fn resolve_fetch_range(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) -> FetchRangeResolution {
        if Self::explicit_end_before_or_equal_start(start, end) {
            return FetchRangeResolution::InvalidRange;
        }

        let Some(largest) = self.largest_location().await else {
            return FetchRangeResolution::NotCovered;
        };

        if Self::location_cmp(start, largest).is_gt()
            && self.live_ingest_count.load(AtomicOrdering::Relaxed) > 0
        {
            return FetchRangeResolution::InvalidRange;
        }

        let end_location = self.resolve_fetch_end_location(end, largest).await;
        if !self.covers(start, end_location).await {
            return FetchRangeResolution::NotCovered;
        }
        if !self.has_fetch_object(start, end_location).await {
            return FetchRangeResolution::NoObjects;
        }

        FetchRangeResolution::Serve { end_location }
    }

    pub(crate) async fn insert_fetch_known_range(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) {
        self.known_ranges.write().await.insert(start, end);
    }

    pub(crate) async fn covers(&self, start: moqt::Location, end: moqt::Location) -> bool {
        self.known_ranges.read().await.contains_range(start, end)
    }

    pub(crate) fn begin_live_ingest(&self) {
        self.live_ingest_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    pub(crate) fn end_live_ingest(&self) {
        let _ = self.live_ingest_count.fetch_update(
            AtomicOrdering::Relaxed,
            AtomicOrdering::Relaxed,
            |count| Some(count.saturating_sub(1)),
        );
    }

    fn location_after_object(group_id: u64, object_id: u64) -> moqt::Location {
        moqt::Location {
            group_id,
            object_id: object_id.saturating_add(1),
        }
    }

    fn location_cmp(left: moqt::Location, right: moqt::Location) -> Ordering {
        (left.group_id, left.object_id).cmp(&(right.group_id, right.object_id))
    }

    fn explicit_end_before_or_equal_start(start: moqt::Location, end: moqt::Location) -> bool {
        start.group_id > end.group_id
            || (start.group_id == end.group_id
                && end.object_id != 0
                && start.object_id >= end.object_id)
    }

    async fn resolve_fetch_end_location(
        &self,
        requested_end: moqt::Location,
        largest: moqt::Location,
    ) -> moqt::Location {
        if requested_end.group_id > largest.group_id
            || (requested_end.group_id == largest.group_id
                && requested_end.object_id != 0
                && requested_end.object_id > largest.object_id.saturating_add(1))
        {
            return Self::location_after_object(largest.group_id, largest.object_id);
        }

        if requested_end.object_id == 0 {
            if requested_end.group_id < largest.group_id
                || self.stream_group_is_closed(requested_end.group_id).await
            {
                return requested_end;
            }
            return Self::location_after_object(largest.group_id, largest.object_id);
        }

        requested_end
    }

    async fn stream_group_is_closed(&self, group_id: u64) -> bool {
        let caches = self.stream_group_caches(group_id).await;
        !caches.is_empty() && Self::all_subgroups_closed_for_group(&caches).await
    }

    async fn stream_group_caches(&self, group_id: u64) -> Vec<Arc<GroupCache>> {
        self.stream_groups
            .read()
            .await
            .get(&group_id)
            .map(|subgroups| subgroups.values().cloned().collect())
            .unwrap_or_default()
    }

    async fn all_subgroups_closed_for_group(caches: &[Arc<GroupCache>]) -> bool {
        for cache in caches {
            if cache.header().await.is_none() || !cache.is_closed().await {
                return false;
            }
        }
        true
    }

    async fn has_fetch_object(&self, start: moqt::Location, end: moqt::Location) -> bool {
        let groups_in_range: Vec<(u64, Vec<Arc<GroupCache>>)> = {
            let groups = self.stream_groups.read().await;
            groups
                .range(start.group_id..=end.group_id)
                .map(|(&group_id, subgroups)| {
                    (group_id, subgroups.values().cloned().collect::<Vec<_>>())
                })
                .collect()
        };

        for (group_id, caches) in groups_in_range {
            let start_object_id = if group_id == start.group_id {
                start.object_id
            } else {
                0
            };
            let end_exclusive = if group_id == end.group_id && end.object_id != 0 {
                Some(end.object_id)
            } else {
                None
            };
            for cache in caches {
                if cache
                    .has_object_in_range(start_object_id, end_exclusive)
                    .await
                {
                    return true;
                }
            }
        }

        false
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
    use std::time::Duration;

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

    fn make_status_object(status: moqt::ObjectStatus) -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            subgroup_object: SubgroupObject::new_status(u8::from(status) as u64),
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

    #[tokio::test(start_paused = true)]
    async fn evict_removes_closed_group_after_objects_drain() {
        // Arrange: a closed group whose objects will all expire, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        fill_group(&cache, 0, 3).await;
        cache.close_stream_subgroup(0, &subgroup).await;
        // Act: at t=11s, all objects have drained via object TTL
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl).await;
        // Assert: the closed, drained group (header included) is reclaimed
        assert!(!cache.has_stream_group(0).await);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_open_group_without_objects() {
        // Arrange: an open group with only a header, no objects yet, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        // Act: long past the TTL, but the group is still open
        tokio::time::advance(Duration::from_secs(100)).await;
        cache.evict(ttl).await;
        // Assert: an open group is never evicted, so no resurrection can occur
        assert!(cache.has_stream_group(0).await);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_closed_group_with_remaining_objects() {
        // Arrange: a closed group whose objects are still within TTL
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        fill_group(&cache, 0, 3).await;
        cache.close_stream_subgroup(0, &subgroup).await;
        // Act: at t=5s, the objects have not drained yet
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict(ttl).await;
        // Assert: the group survives until its objects drain
        assert!(cache.has_stream_group(0).await);
        assert_eq!(cache.largest_location().await.unwrap().object_id, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_removes_only_deleted_object_range() {
        // Arrange: object 0 expires before object 1, leaving the cache head missing.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        cache
            .append_live_stream_object(0, &subgroup, Some(0), make_object(0))
            .await;
        tokio::time::advance(Duration::from_secs(6)).await;
        cache
            .append_live_stream_object(0, &subgroup, Some(1), make_object(0))
            .await;

        // Act: only object 0 is older than the TTL.
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict(ttl).await;

        // Assert: a fetch spanning the evicted head is no longer covered locally.
        let head_missing = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 2,
                },
            )
            .await;
        assert_eq!(head_missing, FetchRangeResolution::NotCovered);

        // Assert: the remaining covered suffix can still be served locally.
        let covered_suffix = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 1,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 2,
                },
            )
            .await;
        assert_eq!(
            covered_suffix,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 0,
                    object_id: 2
                }
            }
        );
    }

    #[tokio::test]
    async fn covers_live_range_from_first_ingested_object_to_open_largest_group() {
        // Arrange: live ingestion started at group 3, group 3 and 4 are closed,
        // and group 5 is the currently open largest group.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        fill_group_with_ids(&cache, 3, &[0]).await;
        cache.close_stream_subgroup(3, &subgroup).await;
        fill_group_with_ids(&cache, 4, &[0]).await;
        cache.close_stream_subgroup(4, &subgroup).await;
        fill_group_with_ids(&cache, 5, &[0]).await;

        // Act / Assert: live coverage starts at group 3 and includes the open largest group.
        assert!(
            cache
                .covers(
                    moqt::Location {
                        group_id: 3,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 5,
                        object_id: 1
                    },
                )
                .await
        );
        assert!(
            !cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 5,
                        object_id: 1
                    },
                )
                .await
        );
    }

    #[tokio::test]
    async fn covers_fetch_known_range_after_upstream_fetch_completes() {
        // Arrange: an upstream FETCH completed through group 3.
        let cache = TrackCache::new();
        cache
            .insert_fetch_known_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 3,
                    object_id: 0,
                },
            )
            .await;

        // Act / Assert: the completed FETCH range is now known even without live coverage.
        assert!(
            cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 3,
                        object_id: 0
                    },
                )
                .await
        );
    }

    #[tokio::test]
    async fn covers_merged_fetch_and_live_ranges() {
        // Arrange: upstream FETCH filled groups 0..2, then live ingest reached group 5.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .insert_fetch_known_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 2,
                    object_id: 0,
                },
            )
            .await;
        fill_group_with_ids(&cache, 3, &[0]).await;
        cache.close_stream_subgroup(3, &subgroup).await;
        fill_group_with_ids(&cache, 4, &[0]).await;
        cache.close_stream_subgroup(4, &subgroup).await;
        fill_group_with_ids(&cache, 5, &[0]).await;

        // Act / Assert: adjacent fetch-filled and live ranges form one known range.
        assert!(
            cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 5,
                        object_id: 1
                    },
                )
                .await
        );
    }

    #[tokio::test]
    async fn covers_returns_false_when_merged_ranges_do_not_reach_unknown_tail() {
        // Arrange: upstream FETCH filled groups 0..2, then live ingest reached group 5.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .insert_fetch_known_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 2,
                    object_id: 0,
                },
            )
            .await;
        fill_group_with_ids(&cache, 3, &[0]).await;
        cache.close_stream_subgroup(3, &subgroup).await;
        fill_group_with_ids(&cache, 4, &[0]).await;
        cache.close_stream_subgroup(4, &subgroup).await;
        fill_group_with_ids(&cache, 5, &[0]).await;

        // Act / Assert: group 6 was never known.
        assert!(
            !cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 6,
                        object_id: 0
                    },
                )
                .await
        );
    }

    #[tokio::test]
    async fn resolve_fetch_range_uses_fetch_known_range_before_live_coverage_start() {
        // Arrange: live cache started at g3, then an upstream FETCH filled g0..g2.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        fill_group_with_ids(&cache, 3, &[0]).await;
        cache.close_stream_subgroup(3, &subgroup).await;
        fill_group_with_ids(&cache, 0, &[0]).await;
        cache
            .insert_fetch_known_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 3,
                    object_id: 0,
                },
            )
            .await;

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 3,
                    object_id: 0,
                },
            )
            .await;

        // Assert: the filled historical range is locally servable on later FETCHes.
        assert_eq!(
            resolution,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 3,
                    object_id: 0
                }
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn evicting_newer_location_keeps_fresh_fetch_known_range() {
        // Arrange: a high-location live object is older than a low-location fetched range.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 5, &[0]).await;
        tokio::time::advance(Duration::from_secs(6)).await;
        cache
            .insert_fetch_known_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 2,
                    object_id: 0,
                },
            )
            .await;

        // Act: only the live g5 object is past TTL.
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict(ttl).await;

        // Assert: removing g5 does not erase the fresh fetched historical range.
        assert!(
            cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 2,
                        object_id: 0
                    },
                )
                .await
        );
    }

    #[tokio::test]
    async fn resolve_fetch_range_returns_not_covered_for_start_after_largest_without_live_ingest() {
        // Arrange: the relay has cached objects but no active live ingest to trust Largest.
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0]).await;

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 1,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 2,
                },
            )
            .await;

        // Assert: without live ingest, the relay forwards upstream instead of asserting invalidity.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test]
    async fn resolve_fetch_range_uses_live_ingest_counter_for_start_after_largest() {
        // Arrange: two live ingest paths are active and one ends.
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0]).await;
        cache.begin_live_ingest();
        cache.begin_live_ingest();
        cache.end_live_ingest();

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 1,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 2,
                },
            )
            .await;

        // Assert: one remaining live ingest is enough to trust Largest.
        assert_eq!(resolution, FetchRangeResolution::InvalidRange);
    }

    #[tokio::test]
    async fn covers_returns_false_for_datagram_only_objects() {
        // Arrange: datagrams are cached for delivery but cannot prove that gaps
        // are object non-existence.
        let cache = TrackCache::new();
        cache
            .append_datagram_object(0, Some(1), make_object(0))
            .await;
        cache
            .append_datagram_object(0, Some(3), make_object(0))
            .await;
        cache.close_datagram_group(0).await;

        // Act / Assert: datagram-only cache contents never establish fetch coverage.
        assert!(
            !cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 1
                    },
                    moqt::Location {
                        group_id: 0,
                        object_id: 4
                    },
                )
                .await
        );
        assert!(
            !cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 3
                    },
                    moqt::Location {
                        group_id: 0,
                        object_id: 4
                    },
                )
                .await
        );
    }

    #[tokio::test(start_paused = true)]
    async fn evict_removes_fetch_known_range_for_deleted_objects() {
        // Arrange: a completed upstream FETCH made [g0:o0, g3:o0) known,
        // but the cached object at the head later expires.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0]).await;
        cache
            .insert_fetch_known_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 3,
                    object_id: 0,
                },
            )
            .await;

        // Act: object g0:o0 expires.
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl).await;

        // Assert: coverage can no longer claim the deleted head object.
        assert!(
            !cache
                .covers(
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    },
                    moqt::Location {
                        group_id: 1,
                        object_id: 0
                    },
                )
                .await
        );
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
            .append_live_stream_object(group_id, &subgroup, None, make_header(group_id))
            .await;
        for &id in object_ids {
            cache
                .append_live_stream_object(group_id, &subgroup, Some(id), make_object(0))
                .await;
        }
    }

    fn object_ids(objects: &[moqt::FetchObjectField]) -> Vec<(u64, u64)> {
        objects.iter().map(|o| (o.group_id, o.object_id)).collect()
    }

    #[tokio::test]
    async fn resolve_fetch_range_returns_not_covered_for_empty_cache() {
        // Arrange: empty cache
        let cache = TrackCache::new();
        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 1,
                },
            )
            .await;
        // Assert: no cached objects means the relay cannot safely FIN a fetch stream.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test]
    async fn resolve_fetch_range_accepts_explicit_range() {
        // Arrange: group 0 has exactly the requested object coverage.
        let cache = TrackCache::new();
        fill_group(&cache, 0, 3).await;
        // Act
        let resolution = cache
            .resolve_fetch_range(
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
        // Assert: the explicit range [0, 3) can be served as requested.
        assert_eq!(
            resolution,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 0,
                    object_id: 3
                }
            }
        );
    }

    #[tokio::test]
    async fn resolve_fetch_range_accepts_gapped_objects_with_known_coverage() {
        // Arrange: object 1 is not cached, but cache coverage starts before it.
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0, 2]).await;
        // Act
        let resolution = cache
            .resolve_fetch_range(
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
        // Assert: coverage, not local object-id contiguity, decides servability.
        assert_eq!(
            resolution,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 0,
                    object_id: 3
                }
            }
        );
    }

    #[tokio::test]
    async fn resolve_fetch_range_rejects_missing_group() {
        // Arrange: group 1 is absent between cached group 0 and requested group 2.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        fill_group(&cache, 0, 1).await;
        cache.close_stream_subgroup(0, &subgroup).await;
        fill_group(&cache, 2, 1).await;
        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 2,
                    object_id: 1,
                },
            )
            .await;
        // Assert: local FIN would imply group 1 has no objects.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test]
    async fn resolve_fetch_range_clamps_entire_open_largest_group() {
        // Arrange: end.object_id == 0 requests the full group, but it is still open.
        let cache = TrackCache::new();
        fill_group(&cache, 0, 3).await;
        // Act
        let resolution = cache
            .resolve_fetch_range(
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
        // Assert: unpublished tail objects are not fetched; End Location is largest + 1.
        assert_eq!(
            resolution,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 0,
                    object_id: 3
                }
            }
        );
    }

    #[tokio::test]
    async fn resolve_fetch_range_accepts_entire_closed_group() {
        // Arrange: end.object_id == 0 requests the full group, and it is closed.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        fill_group(&cache, 0, 3).await;
        cache.close_stream_subgroup(0, &subgroup).await;
        // Act
        let resolution = cache
            .resolve_fetch_range(
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
        // Assert: all currently requested group coverage is known locally.
        assert_eq!(
            resolution,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 0,
                    object_id: 0
                }
            }
        );
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
    async fn get_fetch_objects_preserves_status_objects() {
        // Arrange: a status object carries positive knowledge about non-existence.
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        cache
            .append_stream_object(
                0,
                &subgroup,
                Some(2),
                make_status_object(moqt::ObjectStatus::DoesNotExist),
            )
            .await;

        // Act
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

        // Assert: the status object is returned rather than filtered out.
        assert_eq!(object_ids(&objects), vec![(0, 2)]);
        assert!(matches!(
            objects[0].fetch_object,
            moqt::FetchObject::Status(moqt::ObjectStatus::DoesNotExist)
        ));
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
