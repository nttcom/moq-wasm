use std::sync::Arc;

use crate::modules::data_plane::{
    cache::subgroup_key::SubgroupKey,
    cache::{
        cached_object::CachedObject,
        track_cache::ledger::{Ledger, OpenSubgroup},
    },
};

use super::{TrackCache, TrackMalformed, location};

const FINISHED_SUBGROUP_EPOCH: u64 = 0;

/// Live-ingest ownership of one subgroup; dropping it closes the subgroup so
/// every exit path of a reader closes exactly once. Only `finish` marks the
/// subgroup complete (upstream FIN or End of Group); a plain drop — reset,
/// stop, decode error, task abort — leaves its tail unknown (draft-14 §10.4.3).
/// Every publisher of a track sends the same objects (§2.1), so the first
/// stream to finish a subgroup completes it for all of them; the subgroup is
/// aborted only when its last stream ends without finishing.
pub(crate) struct OpenSubgroupGuard<'a> {
    cache: &'a TrackCache,
    key: SubgroupKey,
    epoch: u64,
    finished: bool,
}

impl OpenSubgroupGuard<'_> {
    pub(crate) fn insert(&self, object: CachedObject) -> Result<(), TrackMalformed> {
        self.cache.insert_live(object)
    }

    pub(crate) fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for OpenSubgroupGuard<'_> {
    fn drop(&mut self) {
        self.cache
            .close_subgroup(self.key, self.epoch, self.finished);
    }
}

/// One upstream delivery of a subgroup: generation 0 is its first live stream,
/// and every live stream reopening it after an abort starts the next generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubgroupRun {
    pub(crate) key: SubgroupKey,
    pub(crate) generation: usize,
    pub(crate) first_object_id: u64,
}

#[derive(Debug)]
pub(crate) enum NextObject {
    Object(Arc<CachedObject>),
    Finished,
    Aborted,
}

#[cfg(test)]
impl NextObject {
    pub(crate) fn unwrap(self) -> Arc<CachedObject> {
        match self {
            Self::Object(object) => object,
            other => panic!("expected an object, got {other:?}"),
        }
    }
}

impl TrackCache {
    /// A stream subgroup some publisher already finished stays finished: a later
    /// stream for it only re-delivers objects that are already complete.
    pub(crate) fn open_subgroup(&self, key: SubgroupKey) -> OpenSubgroupGuard<'_> {
        let (run, epoch) = {
            let mut guard = self.write();
            let ledger: &mut Ledger = &mut guard;
            if ledger.finished_subgroups.contains(&key) {
                return OpenSubgroupGuard {
                    cache: self,
                    key,
                    epoch: FINISHED_SUBGROUP_EPOCH,
                    finished: false,
                };
            }
            ledger.start_run_if_reopening_aborted(key);
            let next_open_epoch = &mut ledger.next_open_epoch;
            let open = ledger
                .live_groups
                .entry(key.group_id())
                .or_default()
                .open_subgroups
                .entry(key)
                .or_insert_with(|| {
                    *next_open_epoch += 1;
                    OpenSubgroup {
                        guards: 0,
                        epoch: *next_open_epoch,
                    }
                });
            open.guards += 1;
            let epoch = open.epoch;
            (ledger.latest_run(key), epoch)
        };
        self.notify.notify_waiters();
        let _ = self.subgroup_opened_sender.send(run);
        OpenSubgroupGuard {
            cache: self,
            key,
            epoch,
            finished: false,
        }
    }

    fn close_subgroup(&self, key: SubgroupKey, epoch: u64, finished: bool) {
        {
            let mut guard = self.write();
            let ledger: &mut Ledger = &mut guard;
            let group_id = key.group_id();
            let Some(live) = ledger.live_groups.get_mut(&group_id) else {
                return;
            };
            let Some(open) = live
                .open_subgroups
                .get_mut(&key)
                .filter(|open| open.epoch == epoch)
            else {
                return;
            };
            open.guards -= 1;
            let finishes_every_stream = finished && matches!(key, SubgroupKey::Stream { .. });
            if open.guards > 0 && !finishes_every_stream {
                return;
            }
            live.open_subgroups.remove(&key);
            let stream_group_closed =
                matches!(key, SubgroupKey::Stream { .. }) && !live.has_open_stream();
            let knowledge_frontier = live.knowledge_frontier;
            if live.open_subgroups.is_empty() {
                ledger.live_groups.remove(&group_id);
            }
            if finishes_every_stream {
                ledger.finished_subgroups.insert(key);
            }
            if !finished {
                ledger.aborted_subgroups.insert(key);
            }
            let group_complete = stream_group_closed && !ledger.is_group_aborted(group_id);
            if group_complete {
                ledger.known_ranges.insert(
                    location(group_id, knowledge_frontier),
                    location(group_id, 0),
                );
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

    pub(crate) async fn next_subgroup_object_or_wait(
        &self,
        key: SubgroupKey,
        generation: usize,
        from_object_id: u64,
    ) -> Result<NextObject, TrackMalformed> {
        self.wait_until(|ledger| {
            let run_end = ledger.superseded_run_end(key, generation);
            let next_object = ledger
                .next_subgroup_object(key, from_object_id)
                .filter(|object| run_end.is_none_or(|end| object.location.object_id < end));
            match next_object {
                Some(object) => Some(NextObject::Object(object)),
                None if run_end.is_some() => Some(NextObject::Aborted),
                None if ledger.is_open(key) => None,
                None if ledger.aborted_subgroups.contains(&key) => Some(NextObject::Aborted),
                None => Some(NextObject::Finished),
            }
        })
        .await
    }

    pub(super) async fn next_group_object_or_wait(
        &self,
        group_id: u64,
        reopen_count: usize,
        from_object_id: u64,
    ) -> Result<NextObject, TrackMalformed> {
        self.wait_until(|ledger| {
            if ledger.reopen_count_in_group(group_id) != reopen_count {
                return Some(NextObject::Aborted);
            }
            match ledger.next_group_object(group_id, from_object_id) {
                Some(object) => Some(NextObject::Object(object)),
                None if ledger.has_open_subgroup_in_group(group_id) => None,
                None if ledger.is_group_aborted(group_id) => Some(NextObject::Aborted),
                None => Some(NextObject::Finished),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::cached_object::{
        datagram_object, insert_aborted_group, open_group, stream_key, stream_object,
        stream_object_in_subgroup,
    };

    #[tokio::test]
    async fn next_subgroup_object_or_wait_returns_exact_match() {
        // Arrange: objects at ids 0, 3, 5
        let cache = TrackCache::new();
        let _open = open_group(&cache, 0, &[0, 3, 5]);
        // Act
        let object = cache
            .next_subgroup_object_or_wait(stream_key(0), 0, 3)
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
            .next_subgroup_object_or_wait(stream_key(0), 0, 4)
            .await
            .unwrap()
            .unwrap();
        // Assert
        assert_eq!(object.location.object_id, 5);
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_is_finished_when_closed_and_exhausted() {
        // Arrange: one object at id 0, then the subgroup finishes
        let cache = TrackCache::new();
        open_group(&cache, 0, &[0]).finish();
        // Act / Assert
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 1)
                .await,
            Ok(NextObject::Finished)
        ));
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_is_finished_for_never_opened_subgroup() {
        // Arrange: a fetch fill wrote the object without any live stream
        let cache = TrackCache::new();
        let _ = cache.insert(stream_object(0, 0));
        // Act / Assert: nothing will ever close it, so waiting would hang
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 1)
                .await,
            Ok(NextObject::Finished)
        ));
    }

    #[tokio::test]
    async fn next_subgroup_object_or_wait_only_returns_objects_of_its_subgroup() {
        // Arrange: object 1 belongs to subgroup 1, objects 0 and 2 to subgroup 0
        let cache = TrackCache::new();
        let _open = open_group(&cache, 0, &[0, 2]);
        let _ = cache.insert_live(stream_object_in_subgroup(0, 1, 1));
        // Act
        let object = cache
            .next_subgroup_object_or_wait(stream_key(0), 0, 1)
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
            async move {
                cache
                    .next_subgroup_object_or_wait(stream_key(0), 0, 0)
                    .await
            }
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
            .unwrap();
        assert_eq!(object.location.object_id, 0);
    }

    #[tokio::test]
    async fn waiter_ends_when_the_subgroup_closes() {
        // Arrange: the subgroup is open with no objects yet
        let cache = Arc::new(TrackCache::new());
        let open = cache.open_subgroup(stream_key(0));
        let waiter = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .next_subgroup_object_or_wait(stream_key(0), 0, 0)
                    .await
            }
        });
        tokio::task::yield_now().await;
        // Act
        open.finish();
        // Assert
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake on close")
            .unwrap();
        assert!(matches!(result, Ok(NextObject::Finished)));
    }

    #[tokio::test]
    async fn waiter_learns_that_a_dropped_subgroup_was_aborted() {
        // Arrange: the reader ends without a FIN (reset, stop, decode error, task abort)
        let cache = Arc::new(TrackCache::new());
        let open = cache.open_subgroup(stream_key(0));
        let waiter = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .next_subgroup_object_or_wait(stream_key(0), 0, 0)
                    .await
            }
        });
        tokio::task::yield_now().await;
        // Act
        drop(open);
        // Assert
        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake on close")
            .unwrap();
        assert!(matches!(result, Ok(NextObject::Aborted)));
    }

    #[test]
    fn reopening_an_aborted_subgroup_starts_a_run_after_its_cached_objects() {
        // Arrange
        let cache = TrackCache::new();
        insert_aborted_group(&cache, 0, &[0, 1, 2]);
        let mut subgroup_opened_receiver = cache.subscribe_subgroup_opened();
        // Act
        let _reopened = cache.open_subgroup(stream_key(0));
        // Assert
        assert_eq!(
            subgroup_opened_receiver.try_recv(),
            Ok(SubgroupRun {
                key: stream_key(0),
                generation: 1,
                first_object_id: 3,
            })
        );
    }

    #[tokio::test]
    async fn a_stream_opening_a_finished_subgroup_leaves_it_finished() {
        // Arrange
        let cache = TrackCache::new();
        open_group(&cache, 0, &[0]).finish();
        let mut subgroup_opened_receiver = cache.subscribe_subgroup_opened();
        // Act
        let late = cache.open_subgroup(stream_key(0));
        drop(late);
        // Assert
        assert!(subgroup_opened_receiver.try_recv().is_err());
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 1)
                .await,
            Ok(NextObject::Finished)
        ));
    }

    #[tokio::test]
    async fn superseded_run_is_aborted_at_the_first_object_of_the_reopened_run() {
        // Arrange
        let cache = TrackCache::new();
        insert_aborted_group(&cache, 0, &[0, 1, 2]);
        let reopened = cache.open_subgroup(stream_key(0));
        let _ = reopened.insert(stream_object(0, 3));
        // Act
        let superseded_last = cache
            .next_subgroup_object_or_wait(stream_key(0), 0, 2)
            .await;
        let superseded_end = cache
            .next_subgroup_object_or_wait(stream_key(0), 0, 3)
            .await;
        let reopened_first = cache
            .next_subgroup_object_or_wait(stream_key(0), 1, 3)
            .await;
        // Assert
        assert_eq!(superseded_last.unwrap().unwrap().location.object_id, 2);
        assert!(matches!(superseded_end, Ok(NextObject::Aborted)));
        assert_eq!(reopened_first.unwrap().unwrap().location.object_id, 3);
    }

    #[tokio::test]
    async fn reopened_run_that_finishes_is_finished_and_completes_its_group() {
        // Arrange: object 1 was lost with the aborted run
        let cache = TrackCache::new();
        insert_aborted_group(&cache, 0, &[0]);
        let reopened = cache.open_subgroup(stream_key(0));
        let _ = reopened.insert(stream_object(0, 2));
        // Act
        reopened.finish();
        // Assert
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 1, 3)
                .await,
            Ok(NextObject::Finished)
        ));
        assert!(cache.covers(location(0, 2), location(0, 0)));
        assert!(!cache.covers(location(0, 1), location(0, 2)));
    }

    #[tokio::test]
    async fn the_first_publisher_to_finish_a_subgroup_finishes_it_for_all() {
        // Arrange: two publishers deliver the same subgroup (§8.2)
        let cache = TrackCache::new();
        let first = open_group(&cache, 0, &[0]);
        let _second = cache.open_subgroup(stream_key(0));
        // Act
        first.finish();
        // Assert
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 1)
                .await,
            Ok(NextObject::Finished)
        ));
        assert!(cache.covers(location(0, 0), location(0, 1)));
    }

    #[tokio::test]
    async fn a_publisher_resetting_a_subgroup_another_still_delivers_does_not_abort_it() {
        // Arrange
        let cache = TrackCache::new();
        let reset = cache.open_subgroup(stream_key(0));
        let remaining = cache.open_subgroup(stream_key(0));
        // Act
        drop(reset);
        // Assert: still open, so a waiter keeps waiting
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                cache.next_subgroup_object_or_wait(stream_key(0), 0, 0)
            )
            .await
            .is_err()
        );
        remaining.finish();
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 0)
                .await,
            Ok(NextObject::Finished)
        ));
    }

    #[tokio::test]
    async fn a_stream_for_a_finished_subgroup_still_caches_its_objects() {
        // Arrange
        let cache = TrackCache::new();
        open_group(&cache, 0, &[0]).finish();
        let late = cache.open_subgroup(stream_key(0));
        // Act
        let _ = late.insert(stream_object(0, 1));
        // Assert
        assert_eq!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 1)
                .await
                .unwrap()
                .unwrap()
                .location
                .object_id,
            1
        );
    }

    #[tokio::test]
    async fn a_lagging_publisher_resetting_a_finished_subgroup_does_not_abort_it() {
        // Arrange
        let cache = TrackCache::new();
        let finishing = open_group(&cache, 0, &[0]);
        let lagging = cache.open_subgroup(stream_key(0));
        finishing.finish();
        // Act
        drop(lagging);
        // Assert
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 1)
                .await,
            Ok(NextObject::Finished)
        ));
    }

    #[tokio::test]
    async fn a_datagram_group_stays_open_until_every_publisher_moves_on() {
        // Arrange
        let cache = TrackCache::new();
        let key = SubgroupKey::Datagram { group_id: 0 };
        let first = cache.open_subgroup(key);
        let _second = cache.open_subgroup(key);
        // Act
        first.finish();
        // Assert
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                cache.next_subgroup_object_or_wait(key, 0, 0)
            )
            .await
            .is_err()
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
        // Act / Assert: one subgroup finishing leaves the group open
        first.finish();
        assert!(!cache.covers(location(0, 0), location(0, 0)));
        // Act / Assert: the last one finishing completes the group
        second.finish();
        assert!(cache.covers(location(0, 0), location(0, 0)));
    }

    #[test]
    fn an_aborted_subgroup_keeps_the_group_from_completing() {
        // Arrange: subgroup 1 is reset upstream while subgroup 0 finishes cleanly
        let cache = TrackCache::new();
        let finished = open_group(&cache, 0, &[0]);
        let aborted = cache.open_subgroup(SubgroupKey::Stream {
            group_id: 0,
            subgroup_id: 1,
        });
        // Act
        drop(aborted);
        finished.finish();
        // Assert: the tail of the group stays unknown (§10.4.3)
        assert!(!cache.covers(location(0, 1), location(0, 0)));
        assert!(cache.covers(location(0, 0), location(0, 1)));
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
