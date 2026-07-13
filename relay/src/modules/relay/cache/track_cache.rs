use std::{collections::BTreeMap, sync::Arc, time::Duration};

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
            group.evict_expired_objects(ttl).await;
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
            group.evict_expired_objects(ttl).await;
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

    pub(crate) async fn is_empty(&self) -> bool {
        self.stream_groups.read().await.is_empty() && self.datagram_groups.read().await.is_empty()
    }

    /// Non-blocking `is_empty` for the eviction `remove_if` closure, which is
    /// sync and runs under the DashMap shard lock: it re-validates emptiness at
    /// the moment of removal (a writer may have attached, written, and
    /// detached since the async pre-check). With strong_count == 1 checked
    /// first no other Arc exists, so try_read cannot actually be contended;
    /// treating contention as non-empty is a defensive fallback that errs
    /// toward keeping the track.
    pub(crate) fn is_empty_sync(&self) -> bool {
        self.stream_groups
            .try_read()
            .map(|groups| groups.is_empty())
            .unwrap_or(false)
            && self
                .datagram_groups
                .try_read()
                .map(|groups| groups.is_empty())
                .unwrap_or(false)
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
        //
        // QUIC gives no cross-stream ordering, so groups in range may still be
        // in flight; wait on open groups instead of snapshotting (ingress
        // always closes them, bounding the wait).
        let mut fetch_objects = Vec::new();
        for (group_id, cache) in caches_in_range {
            let Some(header) = cache.header_or_wait().await else {
                continue; // closed before a header arrived
            };
            let (publisher_priority, subgroup_id) = match header.as_ref() {
                DataObject::SubgroupHeader(h) => (h.publisher_priority, h.subgroup_id.resolve()),
                _ => {
                    tracing::error!("unexpected: GroupCache header is not a SubgroupHeader");
                    continue;
                }
            };

            let mut next_object_id = if group_id == start.group_id {
                start.object_id
            } else {
                0
            };
            loop {
                // End Location is exclusive (object_id == 0 = entire group);
                // checked before waiting so we never block on objects past it.
                if group_id == end.group_id && end.object_id != 0 && next_object_id >= end.object_id
                {
                    break;
                }
                let Some((current_object_id, object)) =
                    cache.object_from_or_wait(next_object_id).await
                else {
                    break;
                };
                next_object_id = current_object_id + 1;

                // A gap can jump past the exclusive End Location.
                if group_id == end.group_id
                    && end.object_id != 0
                    && current_object_id >= end.object_id
                {
                    break;
                }

                let field = match object.as_ref() {
                    DataObject::SubgroupObject(f) => f,
                    _ => continue,
                };

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
            extension_headers: ExtensionHeaders::default(),
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

    async fn fill_group(cache: &TrackCache, group_id: u64, count: u64) {
        // object_ids 0, 1, ..., count-1
        let object_ids: Vec<u64> = (0..count).collect();
        fill_group_with_ids(cache, group_id, &object_ids).await;
    }

    // get_fetch_objects waits on open groups, so fetch tests use closed ones
    // (like ingress leaves them once the stream ends).
    async fn fill_closed_group_with_ids(cache: &TrackCache, group_id: u64, object_ids: &[u64]) {
        fill_group_with_ids(cache, group_id, object_ids).await;
        cache
            .close_stream_subgroup(group_id, &StreamSubgroupId::Value(0))
            .await;
    }

    async fn fill_closed_group(cache: &TrackCache, group_id: u64, count: u64) {
        let object_ids: Vec<u64> = (0..count).collect();
        fill_closed_group_with_ids(cache, group_id, &object_ids).await;
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
        fill_closed_group(&cache, 0, 5).await;
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
        fill_closed_group(&cache, 0, 5).await;
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
        fill_closed_group_with_ids(&cache, 0, &[0, 3, 5, 8]).await;
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
        fill_closed_group(&cache, 0, 5).await;
        fill_closed_group(&cache, 1, 5).await;
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

    #[tokio::test]
    async fn get_fetch_objects_waits_for_in_flight_group() {
        // Arrange: group 0 is still in flight (header only, open) while group 1
        // has already completed — the cross-stream reordering QUIC allows.
        let cache = Arc::new(TrackCache::new());
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        fill_closed_group(&cache, 1, 5).await;

        // Act: fetch [{0,0}, {1,3}) while group 0's objects have not arrived yet.
        let fetch = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .get_fetch_objects(
                        moqt::Location {
                            group_id: 0,
                            object_id: 0,
                        },
                        moqt::Location {
                            group_id: 1,
                            object_id: 3,
                        },
                    )
                    .await
            }
        });
        // Group 0's objects arrive after the fetch started, then the stream ends.
        tokio::task::yield_now().await;
        for id in 0..3u64 {
            cache
                .append_stream_object(0, &subgroup, Some(id), make_object(0))
                .await;
        }
        cache.close_stream_subgroup(0, &subgroup).await;

        let objects = tokio::time::timeout(Duration::from_secs(5), fetch)
            .await
            .expect("fetch must complete once the in-flight group closes")
            .unwrap();
        // Assert: the late group 0 objects are delivered, not silently dropped.
        assert_eq!(
            object_ids(&objects),
            vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]
        );
    }
}
