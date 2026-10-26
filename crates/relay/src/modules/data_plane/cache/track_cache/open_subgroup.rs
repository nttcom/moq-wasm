use std::sync::{Arc, OnceLock};

use tokio::time::Instant;

use bytes::Bytes;

use crate::modules::data_plane::{
    cache::subgroup_key::SubgroupKey,
    cache::{
        cached_object::CachedObject,
        track_cache::{
            ledger::{Ledger, OpenSubgroup},
            partial_object::{PartialEnd, PartialObject},
        },
    },
};

use super::{TrackCache, TrackMalformed, location};

const FINISHED_SUBGROUP_EPOCH: u64 = 0;

/// Live-ingest ownership of one subgroup; dropping it closes the subgroup so
/// every exit path of a reader closes exactly once. Only `finish` marks the
/// subgroup complete (upstream FIN or End of Group); a plain drop — reset,
/// stop, decode error, task abort — leaves its tail unknown (draft-14 §10.4.3).
/// Every publisher of a track sends the same objects (§2.1), so the FIN of a
/// stream that delivered the subgroup from its earliest object seen completes
/// it for all streams. A stream that joined later started mid-subgroup and
/// proves nothing below its first object, so its FIN only closes itself. The
/// subgroup is aborted when its last stream ends without such a FIN.
pub(crate) struct OpenSubgroupGuard<'a> {
    cache: &'a TrackCache,
    key: SubgroupKey,
    epoch: u64,
    first_object_id: OnceLock<u64>,
    finished: bool,
}

impl<'a> OpenSubgroupGuard<'a> {
    pub(crate) fn insert(&self, object: CachedObject) -> Result<(), TrackMalformed> {
        let object_id = object.object.location.object_id;
        if self.first_object_id.set(object_id).is_ok() {
            self.cache
                .record_stream_start(self.key, self.epoch, object_id);
        }
        self.cache.insert_live(object)
    }

    pub(crate) fn finish(mut self) {
        self.finished = true;
    }

    /// Starts `object` (without its payload) whose `payload_len` bytes follow
    /// through the returned writer. Unless the cache holds the object or
    /// another stream is delivering it, egress can forward it from now on.
    pub(crate) fn begin_object(
        &self,
        object: moqt::object::Object,
        payload_len: usize,
    ) -> PartialObjectWriter<'a> {
        let partial = Arc::new(PartialObject::new(object, payload_len));
        let published = self.cache.publish_partial(&partial);
        PartialObjectWriter {
            cache: self.cache,
            partial,
            published,
            finished: false,
        }
    }
}

/// Receives the payload of one object of a live stream. Dropping it before
/// `finish` aborts the object for the egress streams forwarding it.
pub(crate) struct PartialObjectWriter<'a> {
    cache: &'a TrackCache,
    partial: Arc<PartialObject>,
    published: bool,
    finished: bool,
}

impl PartialObjectWriter<'_> {
    pub(crate) fn append(&self, data: Bytes) {
        self.partial.append(data);
    }

    /// Call after the last piece is appended; returns the object with its
    /// whole payload.
    pub(crate) fn assemble(&self) -> moqt::object::Object {
        self.partial.assemble()
    }

    /// Call once the assembled object is in the cache.
    pub(crate) fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for PartialObjectWriter<'_> {
    fn drop(&mut self) {
        self.partial.end(if self.finished {
            PartialEnd::Complete
        } else {
            PartialEnd::Aborted
        });
        if self.published {
            self.cache.withdraw_partial(&self.partial);
        }
    }
}

impl Drop for OpenSubgroupGuard<'_> {
    fn drop(&mut self) {
        self.cache.close_subgroup(
            self.key,
            self.epoch,
            self.first_object_id.get().copied(),
            self.finished,
        );
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
    /// The next object is still arriving; forward it as it does.
    Partial(Arc<PartialObject>),
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
                    first_object_id: OnceLock::new(),
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
                        earliest_first_object_id: None,
                        any_finished: false,
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
            first_object_id: OnceLock::new(),
            finished: false,
        }
    }

    fn publish_partial(&self, partial: &Arc<PartialObject>) -> bool {
        let at = partial.object.location;
        {
            let mut ledger = self.write();
            if self.is_malformed()
                || ledger.objects.contains_key(&at)
                || ledger.partial_objects.contains_key(&at)
            {
                return false;
            }
            ledger.partial_objects.insert(at, partial.clone());
        }
        self.notify.notify_waiters();
        true
    }

    fn withdraw_partial(&self, partial: &Arc<PartialObject>) {
        {
            let mut ledger = self.write();
            let at = partial.object.location;
            if ledger
                .partial_objects
                .get(&at)
                .is_some_and(|published| Arc::ptr_eq(published, partial))
            {
                ledger.partial_objects.remove(&at);
            }
        }
        self.notify.notify_waiters();
    }

    fn record_stream_start(&self, key: SubgroupKey, epoch: u64, first_object_id: u64) {
        let mut ledger = self.write();
        let Some(open) = ledger
            .live_groups
            .get_mut(&key.group_id())
            .and_then(|live| live.open_subgroups.get_mut(&key))
            .filter(|open| open.epoch == epoch)
        else {
            return;
        };
        open.earliest_first_object_id = Some(
            open.earliest_first_object_id
                .map_or(first_object_id, |earliest| earliest.min(first_object_id)),
        );
    }

    fn close_subgroup(
        &self,
        key: SubgroupKey,
        epoch: u64,
        first_object_id: Option<u64>,
        finished: bool,
    ) {
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
            let is_stream = matches!(key, SubgroupKey::Stream { .. });
            let covers_earliest_object = open
                .earliest_first_object_id
                .is_none_or(|earliest| first_object_id.is_some_and(|first| first <= earliest));
            let finishes_every_stream = finished && is_stream && covers_earliest_object;
            if finished && !is_stream {
                open.any_finished = true;
            }
            if open.guards > 0 && !finishes_every_stream {
                return;
            }
            let completed = finishes_every_stream || open.any_finished;
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
            if !completed {
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
            let in_run = |object_id: u64| run_end.is_none_or(|end| object_id < end);
            let next_object = ledger
                .next_subgroup_object(key, from_object_id, Instant::now())
                .filter(|object| in_run(object.object.location.object_id));
            let next_partial = ledger
                .next_subgroup_partial(key, from_object_id)
                .filter(|partial| in_run(partial.object.location.object_id))
                .filter(|partial| {
                    next_object.as_ref().is_none_or(|object| {
                        partial.object.location.object_id < object.object.location.object_id
                    })
                });
            match (next_object, next_partial) {
                (_, Some(partial)) => Some(NextObject::Partial(partial)),
                (Some(object), None) => Some(NextObject::Object(object)),
                (None, None) if run_end.is_some() => Some(NextObject::Aborted),
                (None, None) if ledger.is_open(key) => None,
                (None, None) if ledger.aborted_subgroups.contains(&key) => {
                    Some(NextObject::Aborted)
                }
                (None, None) => Some(NextObject::Finished),
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
        arriving_object, datagram_object, insert_aborted_group, open_group, stream_key,
        stream_object, stream_object_in_subgroup,
    };

    async fn next_at(cache: &TrackCache, object_id: u64) -> NextObject {
        tokio::time::timeout(
            Duration::from_secs(1),
            cache.next_subgroup_object_or_wait(stream_key(0), 0, object_id),
        )
        .await
        .expect("the next object is decided")
        .unwrap()
    }

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
        assert_eq!(object.object.location.object_id, 3);
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
        assert_eq!(object.object.location.object_id, 5);
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
        assert_eq!(object.object.location.object_id, 2);
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
        assert_eq!(object.object.location.object_id, 0);
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
        assert_eq!(
            superseded_last.unwrap().unwrap().object.location.object_id,
            2
        );
        assert!(matches!(superseded_end, Ok(NextObject::Aborted)));
        assert_eq!(
            reopened_first.unwrap().unwrap().object.location.object_id,
            3
        );
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
                .object
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
    async fn a_stream_that_joined_mid_subgroup_does_not_finish_it_for_an_earlier_stream() {
        // Arrange
        let cache = TrackCache::new();
        let earlier = cache.open_subgroup(stream_key(0));
        let _ = earlier.insert(stream_object(0, 0));
        let _ = earlier.insert(stream_object(0, 1));
        let joined_later = cache.open_subgroup(stream_key(0));
        let _ = joined_later.insert(stream_object(0, 3));
        // Act
        joined_later.finish();
        // Assert: the earlier stream is still delivering, so the subgroup is not finished
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                cache.next_subgroup_object_or_wait(stream_key(0), 0, 4)
            )
            .await
            .is_err()
        );
        drop(earlier);
    }

    #[tokio::test]
    async fn a_mid_subgroup_fin_after_the_earlier_stream_reset_leaves_the_subgroup_aborted() {
        // Arrange
        let cache = TrackCache::new();
        let earlier = cache.open_subgroup(stream_key(0));
        let _ = earlier.insert(stream_object(0, 0));
        let joined_later = cache.open_subgroup(stream_key(0));
        let _ = joined_later.insert(stream_object(0, 3));
        drop(earlier);
        // Act
        joined_later.finish();
        // Assert: object 2 was never received by any stream
        assert!(matches!(
            cache
                .next_subgroup_object_or_wait(stream_key(0), 0, 4)
                .await,
            Ok(NextObject::Aborted)
        ));
    }

    #[tokio::test]
    async fn a_datagram_group_one_publisher_finished_is_not_aborted_by_another_leaving() {
        // Arrange
        let cache = TrackCache::new();
        let key = SubgroupKey::Datagram { group_id: 0 };
        let moved_on = cache.open_subgroup(key);
        let leaving = cache.open_subgroup(key);
        moved_on.finish();
        // Act
        drop(leaving);
        // Assert
        assert!(matches!(
            cache.next_subgroup_object_or_wait(key, 0, 0).await,
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

    #[tokio::test]
    async fn an_object_still_arriving_is_the_next_object() {
        // Arrange
        let cache = TrackCache::new();
        let open = cache.open_subgroup(stream_key(0));
        let _writer = open.begin_object(arriving_object(0, 0), 8);

        // Act
        let next = next_at(&cache, 0).await;

        // Assert
        assert!(
            matches!(&next, NextObject::Partial(partial) if partial.object.location == location(0, 0)),
            "{next:?}"
        );
    }

    #[tokio::test]
    async fn a_completed_object_is_read_from_the_cache_instead() {
        // Arrange
        let cache = TrackCache::new();
        let open = cache.open_subgroup(stream_key(0));
        let writer = open.begin_object(arriving_object(0, 0), 8);
        writer.append(bytes::Bytes::from_static(b"keyframe"));
        open.insert(CachedObject::from_object(writer.assemble()))
            .unwrap();
        writer.finish();

        // Act
        let next = next_at(&cache, 0).await;

        // Assert
        assert_eq!(&next.unwrap().object.payload[..], b"keyframe");
        assert!(cache.read().partial_objects.is_empty());
    }

    #[tokio::test]
    async fn an_object_the_cache_holds_is_not_published_while_arriving_again() {
        // Arrange: one stream cached the object, another stream starts it
        let cache = TrackCache::new();
        let _first = open_group(&cache, 0, &[0]);
        let second = cache.open_subgroup(stream_key(0));
        let _writer = second.begin_object(arriving_object(0, 0), 8);

        // Act
        let next = next_at(&cache, 0).await;

        // Assert
        assert!(matches!(next, NextObject::Object(_)), "{next:?}");
        assert!(cache.read().partial_objects.is_empty());
    }

    #[tokio::test]
    async fn only_the_first_stream_starting_an_object_publishes_it() {
        // Arrange
        let cache = TrackCache::new();
        let first = cache.open_subgroup(stream_key(0));
        let second = cache.open_subgroup(stream_key(0));
        let first_writer = first.begin_object(arriving_object(0, 0), 8);
        first_writer.append(bytes::Bytes::from_static(b"keyf"));
        let second_writer = second.begin_object(arriving_object(0, 0), 8);

        // Act
        drop(second_writer);
        let next = next_at(&cache, 0).await;

        // Assert: the second stream's abort does not withdraw the first's object
        let NextObject::Partial(partial) = next else {
            panic!("expected the first stream's arriving object, got {next:?}");
        };
        assert_eq!(&partial.assemble().payload[..], b"keyf");
    }

    #[tokio::test]
    async fn dropping_the_writer_aborts_and_withdraws_the_object() {
        // Arrange
        let cache = TrackCache::new();
        let open = cache.open_subgroup(stream_key(0));
        let writer = open.begin_object(arriving_object(0, 0), 8);
        let NextObject::Partial(partial) = next_at(&cache, 0).await else {
            panic!("expected the arriving object");
        };
        let progress = partial.subscribe();

        // Act
        drop(writer);

        // Assert
        assert_eq!(progress.borrow().end, Some(PartialEnd::Aborted));
        assert!(cache.read().partial_objects.is_empty());
    }

    #[tokio::test]
    async fn an_object_still_arriving_is_not_the_largest_location() {
        // Arrange
        let cache = TrackCache::new();
        let open = open_group(&cache, 0, &[0]);

        // Act
        let _writer = open.begin_object(arriving_object(0, 1), 8);

        // Assert
        assert_eq!(cache.largest_location(), Some(location(0, 0)));
    }
}
