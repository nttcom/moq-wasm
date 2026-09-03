use std::sync::Arc;

use crate::modules::relay::{
    cache::{cached_object::CachedObject, track_cache::ledger::Ledger},
    types::SubgroupKey,
};

use super::{TrackCache, TrackMalformed, location};

/// Live-ingest ownership of one subgroup; dropping it closes the subgroup so
/// every exit path of a reader (FIN, stop, error, abort) closes exactly once.
pub(crate) struct OpenSubgroupGuard<'a> {
    cache: &'a TrackCache,
    key: SubgroupKey,
}

impl OpenSubgroupGuard<'_> {
    pub(crate) fn insert(&self, object: CachedObject) -> Result<(), TrackMalformed> {
        self.cache.insert_live(object)
    }
}

impl Drop for OpenSubgroupGuard<'_> {
    fn drop(&mut self) {
        self.cache.close_subgroup(self.key);
    }
}

impl TrackCache {
    pub(crate) fn open_subgroup(&self, key: SubgroupKey) -> OpenSubgroupGuard<'_> {
        *self
            .write()
            .live_groups
            .entry(key.group_id())
            .or_default()
            .open_subgroups
            .entry(key)
            .or_default() += 1;
        self.notify.notify_waiters();
        OpenSubgroupGuard { cache: self, key }
    }

    fn close_subgroup(&self, key: SubgroupKey) {
        {
            let mut guard = self.write();
            let ledger: &mut Ledger = &mut guard;
            let group_id = key.group_id();
            let Some(live) = ledger.live_groups.get_mut(&group_id) else {
                return;
            };
            let Some(open_count) = live.open_subgroups.get_mut(&key) else {
                return;
            };
            *open_count -= 1;
            if *open_count > 0 {
                return;
            }
            live.open_subgroups.remove(&key);
            // Once every live subgroup stream of the group has closed, no later
            // subgroup for the group is assumed: the rest of the group becomes
            // known, from the live frontier so evicted positions stay unknown.
            if matches!(key, SubgroupKey::Stream { .. }) && !live.has_open_stream() {
                ledger
                    .known_ranges
                    .insert(live.knowledge_frontier(group_id), location(group_id, 0));
            }
            if live.open_subgroups.is_empty() {
                ledger.live_groups.remove(&group_id);
            }
        }
        self.notify.notify_waiters();
    }

    /// `enable()` registers the waiter before the ledger is read, so a
    /// `notify_waiters` firing between the check and the await cannot be lost.
    async fn wait_until<T>(
        &self,
        mut decide: impl FnMut(&Ledger) -> Option<T>,
    ) -> Result<T, TrackMalformed> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_malformed() {
                return Err(TrackMalformed);
            }
            if let Some(decision) = decide(&self.read()) {
                return Ok(decision);
            }
            tokio::select! {
                _ = notified => {}
                _ = self.malformed_track_detected() => return Err(TrackMalformed),
            }
        }
    }

    /// Next object of `key` with id >= `from_object_id`, waiting while the
    /// subgroup is still open under live ingest. `Ok(None)` once it is closed
    /// and no such object exists; `Err` as soon as the track is malformed.
    pub(crate) async fn next_subgroup_object_or_wait(
        &self,
        key: SubgroupKey,
        from_object_id: u64,
    ) -> Result<Option<Arc<CachedObject>>, TrackMalformed> {
        self.wait_until(
            |ledger| match ledger.next_subgroup_object(key, from_object_id) {
                Some(object) => Some(Some(object)),
                None if ledger.is_open(key) => None,
                None => Some(None),
            },
        )
        .await
    }

    pub(super) async fn next_group_object_or_wait(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> Result<Option<Arc<CachedObject>>, TrackMalformed> {
        self.wait_until(
            |ledger| match ledger.next_group_object(group_id, from_object_id) {
                Some(object) => Some(Some(object)),
                None if ledger.has_open_subgroup_in_group(group_id) => None,
                None => Some(None),
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::modules::relay::tests::harness::fixtures::cached_object::{
        datagram_object, open_group, stream_key, stream_object, stream_object_in_subgroup,
    };

    fn location(group_id: u64, object_id: u64) -> moqt::Location {
        moqt::Location {
            group_id,
            object_id,
        }
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_returns_exact_match() {
        // Arrange: objects at ids 0, 3, 5
        let cache = TrackCache::new();
        let _open = open_group(&cache, 0, &[0, 3, 5]);
        // Act
        let object = cache
            .next_subgroup_object_or_wait(stream_key(0), 3)
            .await
            .unwrap()
            .unwrap();
        // Assert: the exact id is returned (inclusive lower bound)
        assert_eq!(object.location.object_id, 3);
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_skips_gap_to_next_id() {
        // Arrange: objects at ids 0, 3, 5 (no object at 1, 2, 4)
        let cache = TrackCache::new();
        let _open = open_group(&cache, 0, &[0, 3, 5]);
        // Act
        let object = cache
            .next_subgroup_object_or_wait(stream_key(0), 4)
            .await
            .unwrap()
            .unwrap();
        // Assert
        assert_eq!(object.location.object_id, 5);
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_returns_none_when_closed_and_exhausted() {
        // Arrange: one object at id 0, then the subgroup closes
        let cache = TrackCache::new();
        drop(open_group(&cache, 0, &[0]));
        // Act / Assert
        assert!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 1)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_returns_none_for_never_opened_subgroup() {
        // Arrange: a fetch fill wrote the object without any live stream
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0));
        // Act / Assert: nothing will ever close it, so waiting would hang
        assert!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 1)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_only_returns_objects_of_its_subgroup() {
        // Arrange: object 1 belongs to subgroup 1, objects 0 and 2 to subgroup 0
        let cache = TrackCache::new();
        let _open = open_group(&cache, 0, &[0, 2]);
        let _ = cache.insert_live(stream_object_in_subgroup(0, 1, 1));
        // Act
        let object = cache
            .next_subgroup_object_or_wait(stream_key(0), 1)
            .await
            .unwrap()
            .unwrap();
        // Assert
        assert_eq!(object.location.object_id, 2);
    }

    #[tokio::test]
    async fn waiter_receives_object_inserted_while_waiting() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let live_cache = cache.clone();
        let waiter = tokio::spawn({
            let cache = cache.clone();
            async move { cache.next_subgroup_object_or_wait(stream_key(0), 0).await }
        });
        let open = live_cache.open_subgroup(stream_key(0));
        tokio::task::yield_now().await;
        // Act
        let _ = open.insert(stream_object(0, 0));
        // Assert
        let object = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake on insert")
            .unwrap()
            .unwrap()
            .expect("the inserted object is returned");
        assert_eq!(object.location.object_id, 0);
    }

    #[tokio::test]
    async fn waiter_ends_when_the_subgroup_closes() {
        // Arrange: the subgroup is open with no objects yet
        let cache = Arc::new(TrackCache::new());
        let open = cache.open_subgroup(stream_key(0));
        let waiter = tokio::spawn({
            let cache = cache.clone();
            async move { cache.next_subgroup_object_or_wait(stream_key(0), 0).await }
        });
        tokio::task::yield_now().await;
        // Act
        drop(open);
        // Assert
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake on close")
            .unwrap();
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn subgroup_stays_open_until_every_live_stream_closes() {
        // Arrange: two upstream streams deliver the same subgroup (§8.2)
        let cache = TrackCache::new();
        let first = cache.open_subgroup(stream_key(0));
        let second = cache.open_subgroup(stream_key(0));
        // Act
        drop(first);
        // Assert: still open, so a waiter would keep waiting
        assert!(cache.has_group(0));
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                cache.next_subgroup_object_or_wait(stream_key(0), 0)
            )
            .await
            .is_err()
        );
        drop(second);
        assert!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn closing_the_last_stream_subgroup_of_a_group_makes_the_group_known() {
        // Arrange
        let cache = TrackCache::new();
        let first = open_group(&cache, 0, &[0]);
        let second = cache.open_subgroup(SubgroupKey::Stream {
            group_id: 0,
            subgroup_id: 1,
        });
        // Act / Assert: one subgroup closing leaves the group open
        drop(first);
        assert!(!cache.covers(location(0, 0), location(0, 0)));
        // Act / Assert: the last one closing completes the group
        drop(second);
        assert!(cache.covers(location(0, 0), location(0, 0)));
    }

    #[test]
    fn closing_a_datagram_group_does_not_register_knowledge() {
        // Arrange
        let cache = TrackCache::new();
        let key = SubgroupKey::Datagram { group_id: 0 };
        let open = cache.open_subgroup(key);
        let _ = open.insert(datagram_object(0, 0));
        // Act
        drop(open);
        // Assert: datagrams cannot prove gaps are non-existence
        assert!(!cache.covers(location(0, 0), location(0, 1)));
    }
}
