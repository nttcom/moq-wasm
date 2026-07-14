use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering},
    },
    time::Duration,
};

use tokio::sync::RwLock;

use crate::modules::{
    core::data_object::DataObject,
    relay::{
        cache::{
            group_cache::{EntryOrigin, GroupCache},
            known_ranges::KnownRanges,
        },
        types::StreamSubgroupId,
    },
};

pub(crate) struct TrackCache {
    stream_groups: RwLock<BTreeMap<u64, BTreeMap<StreamSubgroupId, Arc<GroupCache>>>>,
    datagram_groups: RwLock<BTreeMap<u64, Arc<GroupCache>>>,
    known_ranges: RwLock<KnownRanges>,
    live_ingest_count: AtomicUsize,
    eviction_generation: AtomicU64,
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
            eviction_generation: AtomicU64::new(0),
        }
    }

    async fn ensure_stream_subgroup(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        origin: EntryOrigin,
    ) -> Arc<GroupCache> {
        let cache = if let Some(existing) = self
            .stream_groups
            .read()
            .await
            .get(&group_id)
            .and_then(|subgroups| subgroups.get(subgroup_id))
            .cloned()
        {
            existing
        } else {
            let mut groups = self.stream_groups.write().await;
            groups
                .entry(group_id)
                .or_default()
                .entry(subgroup_id.clone())
                .or_insert_with(|| Arc::new(GroupCache::new(origin)))
                .clone()
        };
        // Live ingest claims entries a fetch fill may have created first;
        // origins are never demoted.
        if origin == EntryOrigin::Live {
            cache.promote_to_live();
        }
        cache
    }

    async fn ensure_datagram_group(&self, group_id: u64) -> Arc<GroupCache> {
        if let Some(existing) = self.datagram_groups.read().await.get(&group_id).cloned() {
            return existing;
        }

        let mut groups = self.datagram_groups.write().await;
        groups
            .entry(group_id)
            // Datagrams are only ingested live.
            .or_insert_with(|| Arc::new(GroupCache::new(EntryOrigin::Live)))
            .clone()
    }

    pub(crate) async fn append_stream_object(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        object_id: Option<u64>,
        object: DataObject,
    ) {
        let group = self
            .ensure_stream_subgroup(group_id, subgroup_id, EntryOrigin::FetchFill)
            .await;
        group.append(object_id, Arc::new(object)).await;
    }

    pub(crate) async fn append_live_stream_object(
        &self,
        group_id: u64,
        subgroup_id: &StreamSubgroupId,
        object_id: Option<u64>,
        object: DataObject,
    ) {
        let group = self
            .ensure_stream_subgroup(group_id, subgroup_id, EntryOrigin::Live)
            .await;
        group.append(object_id, Arc::new(object)).await;
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
        let group = self
            .ensure_stream_subgroup(group_id, subgroup_id, EntryOrigin::Live)
            .await;
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
        let mut removed_any = false;
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
                removed_any = true;
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
                    removed_any |= subgroups.remove(&subgroup_id).is_some();
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
            if group.evict_expired_objects(ttl).await.is_some() {
                removed_any = true;
            }
            if group.is_evictable().await {
                removable_datagrams.push(group_id);
            }
        }
        if !removable_datagrams.is_empty() {
            let mut groups = self.datagram_groups.write().await;
            for group_id in removable_datagrams {
                removed_any |= groups.remove(&group_id).is_some();
            }
        }
        if removed_any {
            self.eviction_generation
                .fetch_add(1, AtomicOrdering::Relaxed);
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

        if start > largest && self.live_ingest_count.load(AtomicOrdering::Relaxed) > 0 {
            return FetchRangeResolution::InvalidRange;
        }

        let end_location = self.resolve_fetch_end_location(end, largest).await;
        if self.covers(start, end_location).await {
            if !self.has_fetch_object(start, end_location).await {
                return FetchRangeResolution::NoObjects;
            }
            return FetchRangeResolution::Serve { end_location };
        }

        // In-flight tolerance: QUIC gives no cross-stream ordering, so a
        // group's FIN can lag behind its objects (and behind later groups).
        // If live ingest is running and every group in range has knowledge
        // accumulating from its head, the data is arriving and only closes
        // are pending — serve and let delivery wait (bounded: live ingress
        // always closes its groups). This is the relay-side §9.16 pause;
        // without it these fetches would be forwarded upstream, which ends
        // at a publisher client that cannot serve FETCH.
        if self.serves_after_in_flight_wait(start, end_location).await
            && self.has_fetch_object(start, end_location).await
        {
            return FetchRangeResolution::Serve { end_location };
        }

        FetchRangeResolution::NotCovered
    }

    /// True when the only missing knowledge in [start, end) is open tails of
    /// live groups. Each group must have knowledge from object 0 (per-append
    /// inserts produce exactly that for live data): a group with no knowledge
    /// island, or one not starting at its head (evicted prefix, fetch-fill
    /// leftovers), disqualifies the range.
    async fn serves_after_in_flight_wait(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) -> bool {
        if self.live_ingest_count.load(AtomicOrdering::Relaxed) == 0 {
            return false;
        }
        let known_ranges = self.known_ranges.read().await;
        for group_id in start.group_id..=end.group_id {
            if known_ranges
                .end_of_range_containing(moqt::Location {
                    group_id,
                    object_id: 0,
                })
                .is_none()
            {
                return false;
            }
        }
        true
    }

    pub(crate) async fn insert_fetch_known_range(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) {
        self.known_ranges.write().await.insert(start, end);
    }

    pub(crate) fn eviction_generation(&self) -> u64 {
        self.eviction_generation.load(AtomicOrdering::Relaxed)
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

    #[cfg(test)]
    pub(crate) async fn get_fetch_objects(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) -> Vec<moqt::FetchObjectField> {
        self.get_fetch_objects_with_group_order(start, end, moqt::GroupOrder::Ascending)
            .await
    }

    pub(crate) async fn get_fetch_objects_with_group_order(
        &self,
        start: moqt::Location,
        end: moqt::Location,
        group_order: moqt::GroupOrder,
    ) -> Vec<moqt::FetchObjectField> {
        let mut groups_in_range: Vec<(u64, Vec<Arc<GroupCache>>)> = {
            // NOTE: Datagram objects are not included.
            let groups = self.stream_groups.read().await;
            groups
                .range(start.group_id..=end.group_id)
                .map(|(&group_id, subgroup_map)| {
                    (group_id, subgroup_map.values().cloned().collect())
                })
                .collect()
        };
        if matches!(group_order, moqt::GroupOrder::Descending) {
            groups_in_range.reverse();
        }

        // Convert each group into FetchObjectFields, honoring the requested
        // group order; within a group, objects are emitted in object_id order
        // regardless of subgroup (spec: subgroup ID is not used for ordering).
        //
        // QUIC gives no cross-stream ordering, so groups in range may still be
        // in flight. Positions inside the track's known ranges are read without
        // waiting (absence there means the object does not exist — this is what
        // keeps never-closed fetch-filled groups from blocking); past that
        // knowledge frontier we wait like live egress does, bounded because
        // live ingress always closes its groups and the exclusive End Location
        // is checked before each wait.
        let mut fetch_objects = Vec::new();
        for (group_id, caches) in groups_in_range {
            let known_prefix_end =
                self.known_ranges
                    .read()
                    .await
                    .end_of_range_containing(moqt::Location {
                        group_id,
                        object_id: 0,
                    });
            let group_fully_known =
                matches!(known_prefix_end, Some(end) if end.group_id > group_id);
            let frontier: Option<u64> = match known_prefix_end {
                Some(end) if end.group_id == group_id => Some(end.object_id),
                _ => None,
            };
            let start_object_id = if group_id == start.group_id {
                start.object_id
            } else {
                0
            };
            let end_exclusive: Option<u64> = if group_id == end.group_id && end.object_id != 0 {
                Some(end.object_id)
            } else {
                None
            };

            let mut group_objects = Vec::new();
            for cache in caches {
                let Some(header) = cache.header_or_wait().await else {
                    continue; // closed before a header arrived
                };
                let (publisher_priority, subgroup_id) = match header.as_ref() {
                    DataObject::SubgroupHeader(h) => {
                        (h.publisher_priority, h.subgroup_id.resolve())
                    }
                    _ => {
                        tracing::error!("unexpected: GroupCache header is not a SubgroupHeader");
                        continue;
                    }
                };

                let mut next_object_id = start_object_id;
                loop {
                    if let Some(end_object_id) = end_exclusive
                        && next_object_id >= end_object_id
                    {
                        break;
                    }
                    let in_known = group_fully_known
                        || frontier.is_some_and(|frontier| next_object_id < frontier);
                    let found = if in_known {
                        cache.first_object_from(next_object_id).await
                    } else {
                        cache.object_from_or_wait(next_object_id).await
                    };
                    match found {
                        Some((current_object_id, object)) => {
                            next_object_id = current_object_id + 1;
                            // A gap can jump past the exclusive End Location.
                            if let Some(end_object_id) = end_exclusive
                                && current_object_id >= end_object_id
                            {
                                break;
                            }
                            Self::push_fetch_object(
                                &mut group_objects,
                                group_id,
                                subgroup_id,
                                publisher_priority,
                                current_object_id,
                                object,
                            );
                        }
                        None if group_fully_known => break,
                        None if in_known => {
                            // Nothing left below the knowledge frontier; move the
                            // cursor there so the next iteration waits like live.
                            next_object_id = frontier.unwrap_or(next_object_id);
                        }
                        // object_from_or_wait: the group closed and no object remains.
                        None => break,
                    }
                }
            }
            group_objects.sort_by_key(|object| object.object_id);
            fetch_objects.extend(group_objects);
        }
        fetch_objects
    }

    fn push_fetch_object(
        group_objects: &mut Vec<moqt::FetchObjectField>,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
        object_id: u64,
        object: Arc<DataObject>,
    ) {
        let DataObject::SubgroupObject(field) = object.as_ref() else {
            return;
        };
        let fetch_object = match &field.subgroup_object {
            moqt::SubgroupObject::Payload { data, .. } => moqt::FetchObject::Payload(data.clone()),
            moqt::SubgroupObject::Status { code, .. } => {
                let Ok(status) = moqt::ObjectStatus::try_from(*code as u8) else {
                    return;
                };
                moqt::FetchObject::Status(status)
            }
        };
        group_objects.push(moqt::FetchObjectField::new(
            group_id,
            subgroup_id,
            object_id,
            publisher_priority,
            field.extension_headers.clone(),
            fetch_object,
        ));
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
    async fn evict_reclaims_empty_fetch_fill_leftover() {
        // Arrange: a fetch fill wrote only a header before failing; the entry
        // never closes, and emptiness alone marks it reclaimable.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        // Act
        cache.evict(ttl).await;
        // Assert: fetch-fill leftovers are reclaimed as soon as they are empty.
        assert!(!cache.has_stream_group(0).await);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_empty_open_live_group() {
        // Arrange: live ingress may send a header long before the first object.
        // Reclaiming that placeholder would resurrect a headerless GroupCache,
        // so live-origin entries are only reclaimed once closed.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_live_stream_object(0, &subgroup, None, make_header(0))
            .await;

        // Act
        tokio::time::advance(Duration::from_secs(100)).await;
        cache.evict(ttl).await;

        // Assert
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

    #[tokio::test(start_paused = true)]
    async fn evict_reclaims_fetch_fill_subgroup_after_objects_expire() {
        // Arrange: fetch-fill writes an unclosed subgroup.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_stream_object(0, &subgroup, None, make_header(0))
            .await;
        cache
            .append_stream_object(0, &subgroup, Some(0), make_object(0))
            .await;

        // Act: the first eviction removes the object; the next pass removes the empty subgroup.
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl).await;
        cache.evict(ttl).await;

        // Assert
        assert!(!cache.has_stream_group(0).await);
    }

    #[tokio::test(start_paused = true)]
    async fn eviction_generation_does_not_increment_when_nothing_removed() {
        // Arrange
        let cache = TrackCache::new();
        let generation = cache.eviction_generation();

        // Act
        cache.evict(Duration::from_secs(10)).await;

        // Assert
        assert_eq!(cache.eviction_generation(), generation);
    }

    #[tokio::test(start_paused = true)]
    async fn eviction_generation_increments_when_objects_are_removed() {
        // Arrange
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0]).await;
        let generation = cache.eviction_generation();

        // Act
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl).await;

        // Assert
        assert!(cache.eviction_generation() > generation);
    }

    #[tokio::test]
    async fn resolve_fetch_range_serves_in_flight_groups_before_close() {
        // Arrange: live ingest delivered g0/g1 objects but their FINs have not
        // been processed yet (QUIC gives no cross-stream ordering).
        let cache = TrackCache::new();
        cache.begin_live_ingest();
        fill_group_with_ids(&cache, 0, &[0, 1, 2]).await;
        fill_group_with_ids(&cache, 1, &[0, 1]).await;

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 2,
                },
            )
            .await;

        // Assert: serve and let delivery wait for the pending closes instead
        // of forwarding upstream.
        assert_eq!(
            resolution,
            FetchRangeResolution::Serve {
                end_location: moqt::Location {
                    group_id: 1,
                    object_id: 2
                }
            }
        );
    }

    #[tokio::test]
    async fn resolve_fetch_range_not_covered_for_open_groups_without_live_ingest() {
        // Arrange: same cache state, but no live ingest is running, so the
        // missing closes will never arrive locally.
        let cache = TrackCache::new();
        fill_group_with_ids(&cache, 0, &[0, 1, 2]).await;
        fill_group_with_ids(&cache, 1, &[0, 1]).await;

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 2,
                },
            )
            .await;

        // Assert
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test(start_paused = true)]
    async fn resolve_fetch_range_not_covered_when_group_head_was_evicted() {
        // Arrange: g0's only object expired and was evicted (knowledge removed),
        // then live ingest continued with g1.
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        cache.begin_live_ingest();
        fill_group_with_ids(&cache, 0, &[0]).await;
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl).await;
        fill_group_with_ids(&cache, 1, &[0]).await;

        // Act: the range includes the evicted head of g0.
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 1,
                },
            )
            .await;

        // Assert: evicted knowledge must not be served as an in-flight wait.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
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
    async fn get_fetch_objects_merges_subgroups_by_object_id() {
        // Arrange: one group split across even and odd object IDs.
        let cache = TrackCache::new();
        let even = StreamSubgroupId::Value(0);
        let odd = StreamSubgroupId::Value(1);
        cache
            .append_stream_object(0, &even, None, make_header(0))
            .await;
        cache
            .append_stream_object(
                0,
                &odd,
                None,
                DataObject::SubgroupHeader(SubgroupHeader::new(
                    0,
                    0,
                    SubgroupId::Value(1),
                    0,
                    false,
                    false,
                )),
            )
            .await;
        for object_id in [0, 2, 4] {
            cache
                .append_stream_object(0, &even, Some(object_id), make_object(0))
                .await;
        }
        for object_id in [1, 3, 5] {
            cache
                .append_stream_object(0, &odd, Some(object_id), make_object(0))
                .await;
        }
        // get_fetch_objects waits on open groups, so close both subgroups first.
        cache.close_stream_subgroup(0, &even).await;
        cache.close_stream_subgroup(0, &odd).await;

        // Act
        let objects = cache
            .get_fetch_objects(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 6,
                },
            )
            .await;

        // Assert
        assert_eq!(
            object_ids(&objects),
            vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5)]
        );
    }

    #[tokio::test]
    async fn get_fetch_objects_descending_groups_keep_objects_ascending() {
        // Arrange
        let cache = TrackCache::new();
        fill_closed_group_with_ids(&cache, 0, &[0, 1]).await;
        fill_closed_group_with_ids(&cache, 1, &[0, 1]).await;

        // Act
        let objects = cache
            .get_fetch_objects_with_group_order(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 2,
                },
                moqt::GroupOrder::Descending,
            )
            .await;

        // Assert
        assert_eq!(object_ids(&objects), vec![(1, 0), (1, 1), (0, 0), (0, 1)]);
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
