use std::{
    collections::{BTreeMap, btree_map::Entry},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use tokio::{
    sync::{Notify, RwLock},
    time::{Duration, Instant},
};

use crate::modules::core::data_object::DataObject;

/// Lifecycle of a subgroup entry. Transitions only move forward (see
/// [`GroupCache::advance_to`]), so the reachable states are exactly:
/// fetch-fill entries stay at `NoCloseSignal` until a live claim, live
/// entries wait at `AwaitingCloseSignal`, and only live ingest reaches
/// `Closed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SubgroupLifecycle {
    /// Created by a fetch fill and never claimed by live ingest: nothing
    /// will ever close it, so emptiness is its only end-of-life signal.
    NoCloseSignal = 0,
    /// Owned by live ingest: stream FIN, EndOfGroup, or reader teardown is
    /// guaranteed to close it. Open and empty means a placeholder whose
    /// objects are still coming.
    AwaitingCloseSignal = 1,
    /// The close signal arrived; no more objects will be appended.
    Closed = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackMalformed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppendStatus {
    Inserted,
    Duplicate,
}

pub(crate) struct GroupCache {
    header: RwLock<Option<Arc<DataObject>>>,
    objects: RwLock<BTreeMap<u64, (Instant, Arc<DataObject>)>>,
    /// [`SubgroupLifecycle`] as its `repr(u8)`, so advancing is a cheap
    /// `fetch_max` on the hot append path.
    lifecycle: AtomicU8,
    notify: Notify,
}

impl GroupCache {
    pub(crate) fn new(lifecycle: SubgroupLifecycle) -> Self {
        Self {
            header: RwLock::new(None),
            objects: RwLock::new(BTreeMap::new()),
            lifecycle: AtomicU8::new(lifecycle as u8),
            notify: Notify::new(),
        }
    }

    /// Moves the lifecycle forward; advancing to an earlier or equal state is
    /// a no-op. Monotonicity matters: a live claim can race a close (e.g. an
    /// append arriving after FIN handling), and must not regress `Closed`.
    pub(crate) fn advance_to(&self, target: SubgroupLifecycle) {
        self.lifecycle.fetch_max(target as u8, Ordering::AcqRel);
    }

    fn lifecycle(&self) -> SubgroupLifecycle {
        match self.lifecycle.load(Ordering::Acquire) {
            0 => SubgroupLifecycle::NoCloseSignal,
            1 => SubgroupLifecycle::AwaitingCloseSignal,
            _ => SubgroupLifecycle::Closed,
        }
    }

    pub(crate) async fn append(
        &self,
        object_id: Option<u64>,
        object: Arc<DataObject>,
    ) -> Result<AppendStatus, TrackMalformed> {
        let result = match object_id {
            None => {
                let mut header = self.header.write().await;
                match header.as_ref() {
                    Some(existing) if existing.matches_immutable_properties(&object) => {
                        Ok(AppendStatus::Duplicate)
                    }
                    Some(_) => Err(TrackMalformed),
                    None => {
                        *header = Some(object);
                        Ok(AppendStatus::Inserted)
                    }
                }
            }
            Some(object_id) => match self.objects.write().await.entry(object_id) {
                Entry::Occupied(existing) => {
                    if existing.get().1.matches_immutable_properties(&object) {
                        Ok(AppendStatus::Duplicate)
                    } else {
                        Err(TrackMalformed)
                    }
                }
                Entry::Vacant(slot) => {
                    slot.insert((Instant::now(), object));
                    Ok(AppendStatus::Inserted)
                }
            },
        };
        self.notify.notify_waiters();
        result
    }

    pub(crate) async fn contains_object(&self, object_id: u64) -> bool {
        self.objects.read().await.contains_key(&object_id)
    }

    pub(crate) async fn evict_expired_objects(&self, ttl: Duration) -> Option<Range<u64>> {
        let mut removed_start = None;
        let mut removed_end = None;
        self.objects
            .write()
            .await
            .retain(|&object_id, (inserted, _)| {
                let keep = inserted.elapsed() <= ttl;
                if !keep {
                    removed_start =
                        Some(removed_start.map_or(object_id, |start: u64| start.min(object_id)));
                    removed_end =
                        Some(removed_end.map_or(object_id, |end: u64| end.max(object_id)));
                }
                keep
            });
        removed_start
            .zip(removed_end)
            .map(|(start, end)| start..end.saturating_add(1))
    }

    pub(crate) async fn is_evictable(&self) -> bool {
        if !self.objects.read().await.is_empty() {
            return false;
        }
        // Only an entry still awaiting its close signal is a placeholder whose
        // objects may yet come — reclaiming it would resurrect a header-less
        // entry later. Closed entries are done, and no-close-signal entries
        // have nothing to wait for, so once drained empty (objects only leave
        // via TTL) both are leftovers.
        self.lifecycle() != SubgroupLifecycle::AwaitingCloseSignal
    }

    pub(crate) async fn header(&self) -> Option<Arc<DataObject>> {
        self.header.read().await.clone()
    }

    pub(crate) async fn largest_object_id(&self) -> Option<u64> {
        self.objects
            .read()
            .await
            .last_key_value()
            .map(|(&id, _)| id)
    }

    /// Non-waiting lookup of the first object with id >= `from`. Used for
    /// positions already covered by track knowledge, where absence means the
    /// object does not exist and waiting would never be satisfied.
    pub(crate) async fn first_object_from(&self, from: u64) -> Option<(u64, Arc<DataObject>)> {
        self.objects
            .read()
            .await
            .range(from..)
            .next()
            .map(|(&id, (_, object))| (id, object.clone()))
    }

    /// Waits until the subgroup header is available, or returns `None` if the group is
    /// closed before it arrives.
    ///
    /// Ordering matters twice here. `enable()` registers the waiter before the
    /// checks so a `notify_waiters` firing between check and await cannot be
    /// lost. `is_closed` is loaded before the content read: appends
    /// happen-before the close, so observing "not closed" first guarantees the
    /// read below sees every object that will ever matter for this wait; the
    /// reverse order can observe a stale empty read plus a fresh close and cut
    /// the subgroup short.
    pub(crate) async fn header_or_wait(&self) -> Option<Arc<DataObject>> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let closed = self.is_closed();
            if let Some(header) = self.header().await {
                return Some(header);
            }
            if closed {
                return None;
            }
            notified.await;
        }
    }

    /// Returns the first object whose id is `>= object_id` (inclusive), waiting if none is
    /// available yet. Returns `None` once the group is closed and no such object exists.
    ///
    /// See `header_or_wait` for why the checks are ordered this way.
    pub(crate) async fn object_from_or_wait(
        &self,
        object_id: u64,
    ) -> Option<(u64, Arc<DataObject>)> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let closed = self.is_closed();
            if let Some((&id, (_, object))) = self.objects.read().await.range(object_id..).next() {
                return Some((id, object.clone()));
            }
            if closed {
                return None;
            }
            notified.await;
        }
    }

    pub(crate) fn mark_end_of_group(&self) {
        self.advance_to(SubgroupLifecycle::Closed);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.lifecycle() == SubgroupLifecycle::Closed
    }

    /// Returns all cached objects in object_id order. The subgroup header is not included.
    /// Test-only: production reads use `object_from_or_wait` since a snapshot can miss
    /// in-flight objects.
    #[cfg(test)]
    pub(crate) async fn objects_snapshot(&self) -> Vec<(u64, Arc<DataObject>)> {
        self.objects
            .read()
            .await
            .iter()
            .map(|(&id, (_, object))| (id, object.clone()))
            .collect()
    }

    pub(crate) async fn has_object_in_range(&self, start: u64, end_exclusive: Option<u64>) -> bool {
        let objects = self.objects.read().await;
        match end_exclusive {
            Some(end_exclusive) => objects.range(start..end_exclusive).next().is_some(),
            None => objects.range(start..).next().is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};
    use std::time::Duration;

    fn payload_object(payload: &[u8]) -> Arc<DataObject> {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        Arc::new(DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object: SubgroupObject::new_payload(Bytes::from(payload.to_vec())),
        }))
    }

    fn payload_of(object: &DataObject) -> Bytes {
        match object {
            DataObject::SubgroupObject(f) => match &f.subgroup_object {
                SubgroupObject::Payload { data, .. } => data.clone(),
                _ => panic!("expected payload"),
            },
            _ => panic!("expected SubgroupObject"),
        }
    }

    fn header_object(publisher_priority: u8) -> Arc<DataObject> {
        Arc::new(DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            0,
            SubgroupId::Value(0),
            publisher_priority,
            false,
            false,
        )))
    }

    #[tokio::test]
    async fn append_identical_duplicate_keeps_first() {
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let first = cache.append(Some(0), payload_object(b"same")).await;
        let second = cache.append(Some(0), payload_object(b"same")).await;
        assert_eq!(first, Ok(AppendStatus::Inserted));
        assert_eq!(second, Ok(AppendStatus::Duplicate));
        let snapshot = cache.objects_snapshot().await;
        assert_eq!(snapshot.len(), 1);
        let (id, object) = &snapshot[0];
        assert_eq!(*id, 0);
        assert_eq!(payload_of(object), Bytes::from_static(b"same"));
    }

    #[tokio::test]
    async fn append_duplicate_with_different_payload_is_malformed() {
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(Some(0), payload_object(b"first")).await;
        let outcome = cache.append(Some(0), payload_object(b"second")).await;
        assert_eq!(outcome, Err(TrackMalformed));
        let snapshot = cache.objects_snapshot().await;
        assert_eq!(payload_of(&snapshot[0].1), Bytes::from_static(b"first"));
    }

    #[tokio::test]
    async fn append_identical_header_twice_keeps_first() {
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let first = cache.append(None, header_object(1)).await;
        let second = cache.append(None, header_object(1)).await;
        assert_eq!(first, Ok(AppendStatus::Inserted));
        assert_eq!(second, Ok(AppendStatus::Duplicate));
        assert!(cache.header().await.is_some());
    }

    #[tokio::test]
    async fn append_header_with_different_priority_is_malformed() {
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(None, header_object(1)).await;
        let outcome = cache.append(None, header_object(2)).await;
        assert_eq!(outcome, Err(TrackMalformed));
        match cache.header().await.unwrap().as_ref() {
            DataObject::SubgroupHeader(h) => assert_eq!(h.publisher_priority, 1),
            _ => panic!("expected SubgroupHeader"),
        }
    }

    #[tokio::test]
    async fn object_from_or_wait_returns_exact_match() {
        // Arrange: objects at ids 0, 3, 5
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(Some(0), payload_object(b"o0")).await;
        let _ = cache.append(Some(3), payload_object(b"o3")).await;
        let _ = cache.append(Some(5), payload_object(b"o5")).await;
        // Act: ask for exactly id 3
        let (id, _) = cache.object_from_or_wait(3).await.unwrap();
        // Assert: the exact id is returned (inclusive lower bound)
        assert_eq!(id, 3);
    }

    #[tokio::test]
    async fn object_from_or_wait_skips_gap_to_next_id() {
        // Arrange: objects at ids 0, 3, 5 (no object at 1, 2, 4)
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(Some(0), payload_object(b"o0")).await;
        let _ = cache.append(Some(3), payload_object(b"o3")).await;
        let _ = cache.append(Some(5), payload_object(b"o5")).await;
        // Act: ask for id 4, which has no object
        let (id, _) = cache.object_from_or_wait(4).await.unwrap();
        // Assert: the next available id (5) is returned across the gap
        assert_eq!(id, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_removes_objects_older_than_ttl() {
        // Arrange: object 0 at t=0, object 1 at t=6s, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(Some(0), payload_object(b"old")).await;
        tokio::time::advance(Duration::from_secs(6)).await;
        let _ = cache.append(Some(1), payload_object(b"new")).await;
        // Act: at t=11s, object 0 is 11s old (>10), object 1 is 5s old (<=10)
        tokio::time::advance(Duration::from_secs(5)).await;
        cache.evict_expired_objects(ttl).await;
        // Assert: only the expired object 0 is removed
        let snapshot = cache.objects_snapshot().await;
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn evict_keeps_object_at_exactly_ttl() {
        // Arrange: object 0 at t=0, TTL=10s
        let ttl = Duration::from_secs(10);
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(Some(0), payload_object(b"o")).await;
        // Act: at exactly t=10s, age == TTL, which is not `> TTL`
        tokio::time::advance(Duration::from_secs(10)).await;
        cache.evict_expired_objects(ttl).await;
        // Assert: the boundary object survives
        assert_eq!(cache.objects_snapshot().await.len(), 1);
    }

    #[tokio::test]
    async fn object_from_or_wait_returns_none_when_closed() {
        // Arrange: one object at id 0, then the group is closed
        let cache = GroupCache::new(SubgroupLifecycle::AwaitingCloseSignal);
        let _ = cache.append(Some(0), payload_object(b"o0")).await;
        cache.mark_end_of_group();
        // Act: ask for id 1, beyond the last object, on a closed group
        let result = cache.object_from_or_wait(1).await;
        // Assert: no object exists and the group is closed, so None
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn lifecycle_never_regresses() {
        // Arrange: a fetch-fill entry that live ingest claims, then closes
        let cache = GroupCache::new(SubgroupLifecycle::NoCloseSignal);
        cache.advance_to(SubgroupLifecycle::AwaitingCloseSignal);
        cache.mark_end_of_group();
        // Act: a late live claim races in after the close
        cache.advance_to(SubgroupLifecycle::AwaitingCloseSignal);
        // Assert: the entry stays closed (and empty+closed stays evictable)
        assert!(cache.is_closed());
        assert!(cache.is_evictable().await);
    }
}
