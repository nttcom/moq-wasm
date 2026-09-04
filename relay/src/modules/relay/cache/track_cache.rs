use std::{
    collections::btree_map::Entry,
    sync::{
        Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering},
    },
    time::Duration,
};

use tokio::sync::Notify;

use crate::modules::relay::{
    cache::cached_object::{CachedObject, DuplicateKind, ForwardingPreference},
    types::SubgroupKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackMalformed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertStatus {
    Inserted,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertOrigin {
    Live,
    Fill,
}

fn location(group_id: u64, object_id: u64) -> moqt::Location {
    moqt::Location {
        group_id,
        object_id,
    }
}

mod fetch;
mod ledger;
mod live;

use ledger::Ledger;
pub(crate) use live::LiveSubgroup;

pub(crate) use fetch::FetchRangeResolution;

pub(crate) struct TrackCache {
    ledger: RwLock<Ledger>,
    notify: Notify,
    live_ingest_count: AtomicUsize,
    eviction_generation: AtomicU64,
    malformed: AtomicBool,
    malformed_notify: Notify,
}

impl TrackCache {
    pub(crate) fn new() -> Self {
        Self {
            ledger: RwLock::new(Ledger::default()),
            notify: Notify::new(),
            live_ingest_count: AtomicUsize::new(0),
            eviction_generation: AtomicU64::new(0),
            malformed: AtomicBool::new(false),
            malformed_notify: Notify::new(),
        }
    }

    // The ledger is never held across an await, so poisoning can only come from
    // a panic inside a critical section; the data is still consistent enough
    // to keep serving.
    fn read(&self) -> RwLockReadGuard<'_, Ledger> {
        self.ledger.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Ledger> {
        self.ledger.write().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn is_malformed(&self) -> bool {
        self.malformed.load(AtomicOrdering::Acquire)
    }

    fn mark_malformed(&self) {
        self.malformed.store(true, AtomicOrdering::Release);
        self.malformed_notify.notify_waiters();
    }

    pub(crate) async fn malformed_track_detected(&self) {
        loop {
            let notified = self.malformed_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_malformed() {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn insert(
        &self,
        object: CachedObject,
        origin: InsertOrigin,
    ) -> Result<InsertStatus, TrackMalformed> {
        if self.is_malformed() {
            return Err(TrackMalformed);
        }
        let location = object.location;
        let registers_knowledge = origin == InsertOrigin::Live
            && matches!(object.forwarding, ForwardingPreference::Subgroup { .. });
        let status = {
            let mut ledger = self.write();
            let status = match ledger.objects.entry(location) {
                Entry::Occupied(existing) => match existing.get().duplicate_kind(&object) {
                    DuplicateKind::Identical => Ok(InsertStatus::Duplicate),
                    DuplicateKind::Conflict => Err(TrackMalformed),
                },
                Entry::Vacant(slot) => {
                    slot.insert(Arc::new(object));
                    Ok(InsertStatus::Inserted)
                }
            };
            if status.is_ok() && registers_knowledge {
                ledger
                    .known_ranges
                    .insert(location.group_id_start(), location.after());
            }
            status
        };
        if status.is_err() {
            self.mark_malformed();
        }
        self.notify.notify_waiters();
        status
    }

    pub(crate) fn has_group(&self, group_id: u64) -> bool {
        self.read().has_group(group_id)
    }

    pub(crate) fn subgroups_in_group(&self, group_id: u64) -> Vec<SubgroupKey> {
        self.read().subgroups_in_group(group_id)
    }

    pub(crate) fn largest_location(&self) -> Option<moqt::Location> {
        self.read().largest_location()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.read().objects.is_empty()
    }

    pub(crate) fn evict(&self, ttl: Duration) {
        let removed_count = {
            let mut ledger = self.write();
            let mut removed = Vec::new();
            ledger.objects.retain(|location, object| {
                let keep = object.received_at.elapsed() <= ttl;
                if !keep {
                    removed.push(*location);
                }
                keep
            });
            for (start, end) in Self::contiguous_runs(&removed) {
                ledger.known_ranges.remove_range(start, end);
            }
            removed.len()
        };
        if removed_count > 0 {
            self.eviction_generation
                .fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    /// Groups ascending, consecutive object ids (within one group) into
    /// half-open ranges so knowledge is released exactly where objects left.
    fn contiguous_runs(sorted: &[moqt::Location]) -> Vec<(moqt::Location, moqt::Location)> {
        let mut runs: Vec<(moqt::Location, moqt::Location)> = Vec::new();
        for &current in sorted {
            match runs.last_mut() {
                Some((start, end))
                    if start.group_id == current.group_id && end.object_id == current.object_id =>
                {
                    *end = current.after();
                }
                _ => runs.push((current, current.after())),
            }
        }
        runs
    }

    pub(crate) fn insert_fetch_known_range(&self, start: moqt::Location, end: moqt::Location) {
        self.write().known_ranges.insert(start, end);
    }

    pub(crate) fn eviction_generation(&self) -> u64 {
        self.eviction_generation.load(AtomicOrdering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn covers(&self, start: moqt::Location, end: moqt::Location) -> bool {
        self.read().known_ranges.contains_range(start, end)
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
}

trait LocationExt {
    fn after(self) -> moqt::Location;
    fn group_id_start(self) -> moqt::Location;
}

impl LocationExt for moqt::Location {
    fn after(self) -> moqt::Location {
        location(self.group_id, self.object_id.saturating_add(1))
    }

    fn group_id_start(self) -> moqt::Location {
        location(self.group_id, 0)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::modules::relay::tests::harness::fixtures::cached_object::{
        datagram_object, insert_closed_live_group, open_live_group, stream_key, stream_object,
        stream_object_in_subgroup, stream_object_with_payload,
    };

    fn payload_at(cache: &TrackCache, group_id: u64, object_id: u64) -> Bytes {
        cache
            .read()
            .objects
            .get(&location(group_id, object_id))
            .map(|object| object.payload.clone())
            .expect("object should be cached")
    }

    #[test]
    fn identical_duplicate_keeps_first() {
        // Arrange
        let cache = TrackCache::new();
        let first = cache.insert(
            stream_object_with_payload(0, 0, Bytes::from_static(b"same")),
            InsertOrigin::Live,
        );
        // Act
        let second = cache.insert(
            stream_object_with_payload(0, 0, Bytes::from_static(b"same")),
            InsertOrigin::Live,
        );
        // Assert
        assert_eq!(first, Ok(InsertStatus::Inserted));
        assert_eq!(second, Ok(InsertStatus::Duplicate));
        assert!(!cache.is_malformed());
    }

    #[test]
    fn conflicting_duplicate_latches_the_track() {
        // Arrange
        let cache = TrackCache::new();
        let _ = cache.insert(
            stream_object_with_payload(0, 0, Bytes::from_static(b"first")),
            InsertOrigin::Live,
        );
        // Act
        let outcome = cache.insert(
            stream_object_with_payload(0, 0, Bytes::from_static(b"second")),
            InsertOrigin::Live,
        );
        // Assert: malformed and sticky; the first object is kept untouched
        assert_eq!(outcome, Err(TrackMalformed));
        assert!(cache.is_malformed());
        assert_eq!(payload_at(&cache, 0, 0), Bytes::from_static(b"first"));
        assert_eq!(
            cache.insert(stream_object(0, 1), InsertOrigin::Live),
            Err(TrackMalformed)
        );
    }

    #[test]
    fn same_object_id_in_two_subgroups_is_malformed() {
        // Arrange
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Live);
        // Act
        let outcome = cache.insert(stream_object_in_subgroup(0, 1, 0), InsertOrigin::Live);
        // Assert
        assert_eq!(outcome, Err(TrackMalformed));
    }

    #[test]
    fn live_insert_registers_knowledge_from_the_group_head() {
        // Arrange
        let cache = TrackCache::new();
        // Act
        let _ = cache.insert(stream_object(3, 2), InsertOrigin::Live);
        // Assert
        assert!(cache.covers(location(3, 0), location(3, 3)));
        assert!(!cache.covers(location(3, 0), location(3, 4)));
    }

    #[test]
    fn fill_insert_does_not_register_knowledge() {
        // Arrange
        let cache = TrackCache::new();
        // Act
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Fill);
        // Assert: only Fetch::End may register the filled range as known
        assert!(!cache.covers(location(0, 0), location(0, 1)));
    }

    #[test]
    fn datagram_insert_does_not_register_knowledge() {
        // Arrange
        let cache = TrackCache::new();
        // Act
        let _ = cache.insert(datagram_object(0, 1), InsertOrigin::Live);
        let _ = cache.insert(datagram_object(0, 3), InsertOrigin::Live);
        // Assert: datagram-only contents never establish fetch coverage
        assert!(!cache.covers(location(0, 1), location(0, 4)));
        assert!(!cache.covers(location(0, 3), location(0, 4)));
    }

    #[test]
    fn largest_location_is_none_when_empty() {
        assert!(TrackCache::new().largest_location().is_none());
    }

    #[test]
    fn largest_location_is_the_highest_object_of_the_highest_group() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 5]);
        insert_closed_live_group(&cache, 2, &[0, 3]);
        let _ = cache.insert(datagram_object(2, 4), InsertOrigin::Live);
        // Act / Assert: datagram objects count too
        assert_eq!(cache.largest_location(), Some(location(2, 4)));
    }

    #[test]
    fn subgroups_in_group_lists_cached_and_open_subgroups() {
        // Arrange
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object_in_subgroup(0, 1, 0), InsertOrigin::Fill);
        let _open = cache.open_live_subgroup(SubgroupKey::Datagram { group_id: 0 });
        // Act / Assert
        assert_eq!(
            cache.subgroups_in_group(0),
            vec![
                SubgroupKey::Stream {
                    group_id: 0,
                    subgroup_id: 1
                },
                SubgroupKey::Datagram { group_id: 0 }
            ]
        );
        assert!(!cache.has_group(1));
    }

    #[tokio::test(start_paused = true)]
    async fn evict_removes_objects_older_than_ttl() {
        // Arrange: object 0 at t=0, object 1 at t=6s, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Live);
        tokio::time::advance(Duration::from_secs(6)).await;
        let _ = cache.insert(stream_object(0, 1), InsertOrigin::Live);
        // Act: at t=11s, object 0 is 11s old (>10), object 1 is 5s old (<=10)
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict(ttl);
        // Assert
        assert!(!cache.read().objects.contains_key(&location(0, 0)));
        assert!(cache.read().objects.contains_key(&location(0, 1)));
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_object_at_exactly_ttl() {
        // Arrange
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Live);
        // Act: age == TTL, which is not `> TTL`
        tokio::time::advance(Duration::from_secs(10)).await;
        cache.evict(ttl);
        // Assert
        assert!(!cache.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn evict_releases_knowledge_only_where_objects_were_removed() {
        // Arrange: object 0 expires before object 1
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Live);
        tokio::time::advance(Duration::from_secs(6)).await;
        let _ = cache.insert(stream_object(0, 1), InsertOrigin::Live);
        // Act
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict(ttl);
        // Assert
        assert!(!cache.covers(location(0, 0), location(0, 1)));
        assert!(cache.covers(location(0, 1), location(0, 2)));
    }

    #[tokio::test(start_paused = true)]
    async fn evicting_newer_location_keeps_fresh_fetch_known_range() {
        // Arrange: a high-location live object is older than a low-location fetched range
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(5, 0), InsertOrigin::Live);
        tokio::time::advance(Duration::from_secs(6)).await;
        cache.insert_fetch_known_range(location(0, 0), location(2, 0));
        // Act: only the live g5 object is past TTL
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict(ttl);
        // Assert
        assert!(cache.covers(location(0, 0), location(2, 0)));
        assert!(!cache.covers(location(5, 0), location(5, 1)));
    }

    #[tokio::test(start_paused = true)]
    async fn evict_removes_fetch_known_range_for_deleted_objects() {
        // Arrange: a completed upstream FETCH made [g0:o0, g3:o0) known
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Fill);
        cache.insert_fetch_known_range(location(0, 0), location(3, 0));
        // Act: object g0:o0 expires
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl);
        // Assert: coverage can no longer claim the deleted head object
        assert!(!cache.covers(location(0, 0), location(1, 0)));
        assert!(cache.covers(location(0, 1), location(3, 0)));
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_an_open_group_visible_after_its_objects_expire() {
        // Arrange: live ingest may pause long after the header; the open subgroup
        // is what keeps egress waiters alive
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0]);
        // Act
        tokio::time::advance(Duration::from_secs(100)).await;
        cache.evict(ttl);
        // Assert
        assert!(cache.is_empty());
        assert!(cache.has_group(0));
        assert_eq!(cache.subgroups_in_group(0), vec![stream_key(0)]);
    }

    #[tokio::test(start_paused = true)]
    async fn eviction_generation_increments_only_when_objects_are_removed() {
        // Arrange
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Live);
        // Act / Assert: nothing expired yet
        cache.evict(ttl);
        assert_eq!(cache.eviction_generation(), 0);
        // Act / Assert: the object expired
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl);
        assert_eq!(cache.eviction_generation(), 1);
    }

    #[test]
    fn contiguous_runs_split_on_gaps_and_group_boundaries() {
        // Arrange
        let removed = [
            location(0, 3),
            location(0, 4),
            location(0, 6),
            location(1, 0),
        ];
        // Act / Assert
        assert_eq!(
            TrackCache::contiguous_runs(&removed),
            vec![
                (location(0, 3), location(0, 5)),
                (location(0, 6), location(0, 7)),
                (location(1, 0), location(1, 1)),
            ]
        );
    }
}
