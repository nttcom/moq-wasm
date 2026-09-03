use std::sync::Arc;

use crate::modules::relay::{
    cache::{cached_object::CachedObject, track_cache::ledger::Ledger},
    types::SubgroupKey,
};

use super::{InsertOrigin, InsertStatus, TrackCache, TrackMalformed, location};

/// Live-ingest ownership of one subgroup; dropping it closes the subgroup so
/// every exit path of a reader (FIN, stop, error, abort) closes exactly once.
pub(crate) struct LiveSubgroup<'a> {
    cache: &'a TrackCache,
    key: SubgroupKey,
}

impl LiveSubgroup<'_> {
    pub(crate) fn insert(&self, object: CachedObject) -> Result<InsertStatus, TrackMalformed> {
        self.cache.insert(object, InsertOrigin::Live)
    }
}

impl Drop for LiveSubgroup<'_> {
    fn drop(&mut self) {
        self.cache.close_live_subgroup(self.key);
    }
}

impl TrackCache {
    pub(crate) fn open_live_subgroup(&self, key: SubgroupKey) -> LiveSubgroup<'_> {
        *self.write().open_subgroups.entry(key).or_default() += 1;
        self.notify.notify_waiters();
        LiveSubgroup { cache: self, key }
    }

    fn close_live_subgroup(&self, key: SubgroupKey) {
        {
            let mut ledger = self.write();
            let Some(open_count) = ledger.open_subgroups.get_mut(&key) else {
                return;
            };
            *open_count -= 1;
            if *open_count > 0 {
                return;
            }
            ledger.open_subgroups.remove(&key);
            // Once every live subgroup stream of the group has closed, no later
            // subgroup for the group is assumed: the whole group becomes known.
            if let SubgroupKey::Stream { group_id, .. } = key
                && !ledger.has_open_stream_in_group(group_id)
            {
                ledger
                    .known_ranges
                    .insert(location(group_id, 0), location(group_id, 0));
            }
        }
        self.notify.notify_waiters();
    }

    /// `enable()` registers the waiter before the ledger is read, so a
    /// `notify_waiters` firing between the check and the await cannot be lost.
    async fn wait_until<T>(&self, mut decide: impl FnMut(&Ledger) -> Option<T>) -> T {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(decision) = decide(&self.read()) {
                return decision;
            }
            notified.await;
        }
    }

    /// Next object of `key` with id >= `from_object_id`, waiting while the
    /// subgroup is still open under live ingest. `None` once it is closed and
    /// no such object exists.
    pub(crate) async fn next_object_or_wait(
        &self,
        key: SubgroupKey,
        from_object_id: u64,
    ) -> Option<Arc<CachedObject>> {
        self.wait_until(|ledger| match ledger.next_object(key, from_object_id) {
            Some(object) => Some(Some(object)),
            None if ledger.open_subgroups.contains_key(&key) => None,
            None => Some(None),
        })
        .await
    }

    pub(super) async fn next_object_in_group_or_wait(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> Option<Arc<CachedObject>> {
        self.wait_until(
            |ledger| match ledger.next_object_in_group(group_id, from_object_id) {
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
        datagram_object, open_live_group, stream_key, stream_object, stream_object_in_subgroup,
    };

    fn location(group_id: u64, object_id: u64) -> moqt::Location {
        moqt::Location {
            group_id,
            object_id,
        }
    }

    #[tokio::test]
    async fn next_object_or_wait_returns_exact_match() {
        // Arrange: objects at ids 0, 3, 5
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0, 3, 5]);
        // Act
        let object = cache.next_object_or_wait(stream_key(0), 3).await.unwrap();
        // Assert: the exact id is returned (inclusive lower bound)
        assert_eq!(object.location.object_id, 3);
    }

    #[tokio::test]
    async fn next_object_or_wait_skips_gap_to_next_id() {
        // Arrange: objects at ids 0, 3, 5 (no object at 1, 2, 4)
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0, 3, 5]);
        // Act
        let object = cache.next_object_or_wait(stream_key(0), 4).await.unwrap();
        // Assert
        assert_eq!(object.location.object_id, 5);
    }

    #[tokio::test]
    async fn next_object_or_wait_returns_none_when_closed_and_exhausted() {
        // Arrange: one object at id 0, then the subgroup closes
        let cache = TrackCache::new();
        drop(open_live_group(&cache, 0, &[0]));
        // Act / Assert
        assert!(cache.next_object_or_wait(stream_key(0), 1).await.is_none());
    }

    #[tokio::test]
    async fn next_object_or_wait_returns_none_for_never_opened_subgroup() {
        // Arrange: a fetch fill wrote the object without any live stream
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Fill);
        // Act / Assert: nothing will ever close it, so waiting would hang
        assert!(cache.next_object_or_wait(stream_key(0), 1).await.is_none());
    }

    #[tokio::test]
    async fn next_object_or_wait_only_returns_objects_of_its_subgroup() {
        // Arrange: object 1 belongs to subgroup 1, objects 0 and 2 to subgroup 0
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0, 2]);
        let _ = cache.insert(stream_object_in_subgroup(0, 1, 1), InsertOrigin::Live);
        // Act
        let object = cache.next_object_or_wait(stream_key(0), 1).await.unwrap();
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
            async move { cache.next_object_or_wait(stream_key(0), 0).await }
        });
        let live = live_cache.open_live_subgroup(stream_key(0));
        tokio::task::yield_now().await;
        // Act
        let _ = live.insert(stream_object(0, 0));
        // Assert
        let object = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake on insert")
            .unwrap()
            .expect("the inserted object is returned");
        assert_eq!(object.location.object_id, 0);
    }

    #[tokio::test]
    async fn waiter_ends_when_the_subgroup_closes() {
        // Arrange: the subgroup is open with no objects yet
        let cache = Arc::new(TrackCache::new());
        let live = cache.open_live_subgroup(stream_key(0));
        let waiter = tokio::spawn({
            let cache = cache.clone();
            async move { cache.next_object_or_wait(stream_key(0), 0).await }
        });
        tokio::task::yield_now().await;
        // Act
        drop(live);
        // Assert
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake on close")
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn subgroup_stays_open_until_every_live_stream_closes() {
        // Arrange: two upstream streams deliver the same subgroup (§8.2)
        let cache = TrackCache::new();
        let first = cache.open_live_subgroup(stream_key(0));
        let second = cache.open_live_subgroup(stream_key(0));
        // Act
        drop(first);
        // Assert: still open, so a waiter would keep waiting
        assert!(cache.has_group(0));
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                cache.next_object_or_wait(stream_key(0), 0)
            )
            .await
            .is_err()
        );
        drop(second);
        assert!(cache.next_object_or_wait(stream_key(0), 0).await.is_none());
    }

    #[test]
    fn closing_the_last_stream_subgroup_of_a_group_makes_the_group_known() {
        // Arrange
        let cache = TrackCache::new();
        let first = open_live_group(&cache, 0, &[0]);
        let second = cache.open_live_subgroup(SubgroupKey::Stream {
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
        let live = cache.open_live_subgroup(key);
        let _ = live.insert(datagram_object(0, 0));
        // Act
        drop(live);
        // Assert: datagrams cannot prove gaps are non-existence
        assert!(!cache.covers(location(0, 0), location(0, 1)));
    }
}
