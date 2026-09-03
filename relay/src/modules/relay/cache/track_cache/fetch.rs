use std::sync::atomic::Ordering as AtomicOrdering;

use super::{Ledger, LocationExt, TrackCache, TrackMalformed, location};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FetchRangeResolution {
    Serve { end_location: moqt::Location },
    InvalidRange,
    NoObjects,
    NotCovered,
}

impl TrackCache {
    pub(crate) fn resolve_fetch_range(
        &self,
        start: moqt::Location,
        end: moqt::Location,
    ) -> FetchRangeResolution {
        if Self::explicit_end_before_or_equal_start(start, end) {
            return FetchRangeResolution::InvalidRange;
        }

        let ledger = self.read();
        let Some(largest) = ledger.largest_location() else {
            return FetchRangeResolution::NotCovered;
        };

        if start > largest && self.live_ingest_count.load(AtomicOrdering::Relaxed) > 0 {
            return FetchRangeResolution::InvalidRange;
        }

        let end_location = Self::resolve_fetch_end_location(&ledger, end, largest);
        if ledger.known_ranges.contains_range(start, end_location) {
            if !ledger.has_object_in(start, end_location) {
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
        if self.serves_after_in_flight_wait(&ledger, start, end_location)
            && ledger.has_object_in(start, end_location)
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
    fn serves_after_in_flight_wait(
        &self,
        ledger: &Ledger,
        start: moqt::Location,
        end: moqt::Location,
    ) -> bool {
        if self.live_ingest_count.load(AtomicOrdering::Relaxed) == 0 {
            return false;
        }
        (start.group_id..=end.group_id).all(|group_id| {
            ledger
                .known_ranges
                .end_of_range_containing(location(group_id, 0))
                .is_some()
        })
    }

    fn explicit_end_before_or_equal_start(start: moqt::Location, end: moqt::Location) -> bool {
        start.group_id > end.group_id
            || (start.group_id == end.group_id
                && end.object_id != 0
                && start.object_id >= end.object_id)
    }

    fn resolve_fetch_end_location(
        ledger: &Ledger,
        requested_end: moqt::Location,
        largest: moqt::Location,
    ) -> moqt::Location {
        if requested_end.group_id > largest.group_id
            || (requested_end.group_id == largest.group_id
                && requested_end.object_id != 0
                && requested_end.object_id > largest.object_id.saturating_add(1))
        {
            return largest.after();
        }

        if requested_end.object_id == 0 {
            let whole_group = location(requested_end.group_id, 0);
            if requested_end.group_id < largest.group_id
                || ledger.known_ranges.contains_range(whole_group, whole_group)
            {
                return requested_end;
            }
            return largest.after();
        }

        requested_end
    }

    /// Objects in [start, end) honoring the requested group order; within a
    /// group, objects come out in object_id order regardless of subgroup.
    ///
    /// QUIC gives no cross-stream ordering, so groups in range may still be
    /// in flight. Positions inside the track's known ranges are read without
    /// waiting (absence there means the object does not exist); past that
    /// knowledge frontier we wait like live egress does, bounded because live
    /// ingress always closes its subgroups and the exclusive End Location is
    /// checked before each wait.
    pub(crate) async fn fetch_objects(
        &self,
        start: moqt::Location,
        end: moqt::Location,
        group_order: moqt::GroupOrder,
    ) -> Result<Vec<moqt::FetchObjectField>, TrackMalformed> {
        let mut groups = self.read().groups_in_range(start.group_id, end.group_id);
        if matches!(group_order, moqt::GroupOrder::Descending) {
            groups.reverse();
        }

        let mut fetch_objects = Vec::new();
        for group_id in groups {
            let known_prefix_end = self
                .read()
                .known_ranges
                .end_of_range_containing(location(group_id, 0));
            let group_fully_known =
                matches!(known_prefix_end, Some(end) if end.group_id > group_id);
            let frontier: Option<u64> = match known_prefix_end {
                Some(end) if end.group_id == group_id => Some(end.object_id),
                _ => None,
            };
            let mut next_object_id = if group_id == start.group_id {
                start.object_id
            } else {
                0
            };
            let end_exclusive: Option<u64> = if group_id == end.group_id && end.object_id != 0 {
                Some(end.object_id)
            } else {
                None
            };

            loop {
                if self.is_malformed() {
                    return Err(TrackMalformed);
                }
                if end_exclusive.is_some_and(|end_object_id| next_object_id >= end_object_id) {
                    break;
                }
                let in_known =
                    group_fully_known || frontier.is_some_and(|frontier| next_object_id < frontier);
                let found = if in_known {
                    self.read().next_object_in_group(group_id, next_object_id)
                } else {
                    tokio::select! {
                        found = self.next_object_in_group_or_wait(group_id, next_object_id) => found,
                        _ = self.malformed_track_detected() => return Err(TrackMalformed),
                    }
                };
                match found {
                    Some(object) => {
                        let object_id = object.location.object_id;
                        next_object_id = object_id.saturating_add(1);
                        // A gap can jump past the exclusive End Location.
                        if end_exclusive.is_some_and(|end_object_id| object_id >= end_object_id) {
                            break;
                        }
                        fetch_objects.push(object.to_fetch_object_field());
                    }
                    None if group_fully_known => break,
                    None if in_known => {
                        // Nothing left below the knowledge frontier; move the
                        // cursor there so the next iteration waits like live.
                        next_object_id = frontier.unwrap_or(next_object_id);
                    }
                    None => break,
                }
            }
        }
        Ok(fetch_objects)
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use moqt::ObjectStatus;

    use super::*;
    use crate::modules::relay::{
        cache::track_cache::InsertOrigin,
        tests::harness::fixtures::cached_object::{
            datagram_object, insert_closed_live_group, open_live_group, status_object, stream_key,
            stream_object, stream_object_in_subgroup,
        },
        types::SubgroupKey,
    };

    fn object_ids(objects: &[moqt::FetchObjectField]) -> Vec<(u64, u64)> {
        objects
            .iter()
            .map(|object| (object.group_id, object.object_id))
            .collect()
    }

    async fn fetch(
        cache: &TrackCache,
        start: moqt::Location,
        end: moqt::Location,
    ) -> Vec<moqt::FetchObjectField> {
        cache
            .fetch_objects(start, end, moqt::GroupOrder::Ascending)
            .await
            .unwrap()
    }

    #[test]
    fn resolve_fetch_range_returns_not_covered_for_empty_cache() {
        // Arrange / Act / Assert: no cached objects means the relay cannot safely FIN a fetch stream
        assert_eq!(
            TrackCache::new().resolve_fetch_range(location(0, 0), location(0, 1)),
            FetchRangeResolution::NotCovered
        );
    }

    #[test]
    fn resolve_fetch_range_accepts_explicit_range() {
        // Arrange: group 0 has exactly the requested object coverage
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0, 1, 2]);
        // Act / Assert
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(0, 3)),
            FetchRangeResolution::Serve {
                end_location: location(0, 3)
            }
        );
    }

    #[test]
    fn resolve_fetch_range_accepts_gapped_objects_with_known_coverage() {
        // Arrange: object 1 is not cached, but cache coverage starts before it
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0, 2]);
        // Act / Assert: coverage, not local object-id contiguity, decides servability
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(0, 3)),
            FetchRangeResolution::Serve {
                end_location: location(0, 3)
            }
        );
    }

    #[test]
    fn resolve_fetch_range_rejects_missing_group() {
        // Arrange: group 1 is absent between cached group 0 and requested group 2
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0]);
        let _live = open_live_group(&cache, 2, &[0]);
        // Act / Assert: local FIN would imply group 1 has no objects
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(2, 1)),
            FetchRangeResolution::NotCovered
        );
    }

    #[test]
    fn resolve_fetch_range_clamps_entire_open_largest_group() {
        // Arrange: end.object_id == 0 requests the full group, but it is still open
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0, 1, 2]);
        // Act / Assert: unpublished tail objects are not fetched; End Location is largest + 1
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(0, 0)),
            FetchRangeResolution::Serve {
                end_location: location(0, 3)
            }
        );
    }

    #[test]
    fn resolve_fetch_range_accepts_entire_closed_group() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 1, 2]);
        // Act / Assert
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(0, 0)),
            FetchRangeResolution::Serve {
                end_location: location(0, 0)
            }
        );
    }

    #[test]
    fn resolve_fetch_range_returns_no_objects_for_a_known_empty_group() {
        // Arrange: group 1 opened and closed without objects between two live groups
        let cache = TrackCache::new();
        let _g0 = open_live_group(&cache, 0, &[0]);
        insert_closed_live_group(&cache, 1, &[]);
        let _g2 = open_live_group(&cache, 2, &[0]);
        // Act / Assert
        assert_eq!(
            cache.resolve_fetch_range(location(1, 0), location(1, 0)),
            FetchRangeResolution::NoObjects
        );
    }

    #[test]
    fn resolve_fetch_range_serves_in_flight_groups_before_close() {
        // Arrange: live ingest delivered g0/g1 objects but their FINs have not
        // been processed yet (QUIC gives no cross-stream ordering)
        let cache = TrackCache::new();
        cache.begin_live_ingest();
        let _g0 = open_live_group(&cache, 0, &[0, 1, 2]);
        let _g1 = open_live_group(&cache, 1, &[0, 1]);
        // Act / Assert: serve and let delivery wait for the pending closes
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(1, 2)),
            FetchRangeResolution::Serve {
                end_location: location(1, 2)
            }
        );
    }

    #[test]
    fn resolve_fetch_range_not_covered_for_open_groups_without_live_ingest() {
        // Arrange: same cache state, but no live ingest is running
        let cache = TrackCache::new();
        let _g0 = open_live_group(&cache, 0, &[0, 1, 2]);
        let _g1 = open_live_group(&cache, 1, &[0, 1]);
        // Act / Assert
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(1, 2)),
            FetchRangeResolution::NotCovered
        );
    }

    #[tokio::test(start_paused = true)]
    async fn resolve_fetch_range_not_covered_when_group_head_was_evicted() {
        // Arrange: g0's only object expired and was evicted, then live ingest continued with g1
        let ttl = Duration::from_secs(10);
        let cache = TrackCache::new();
        cache.begin_live_ingest();
        let _g0 = open_live_group(&cache, 0, &[0]);
        tokio::time::advance(Duration::from_secs(11)).await;
        cache.evict(ttl);
        let _g1 = open_live_group(&cache, 1, &[0]);
        // Act / Assert: evicted knowledge must not be served as an in-flight wait
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(1, 1)),
            FetchRangeResolution::NotCovered
        );
    }

    #[test]
    fn resolve_fetch_range_start_after_largest_depends_on_live_ingest() {
        // Arrange
        let cache = TrackCache::new();
        let _live = open_live_group(&cache, 0, &[0]);
        // Act / Assert: without live ingest the relay forwards upstream instead of asserting invalidity
        assert_eq!(
            cache.resolve_fetch_range(location(0, 1), location(0, 2)),
            FetchRangeResolution::NotCovered
        );
        // Act / Assert: with live ingest Largest is trusted
        cache.begin_live_ingest();
        cache.begin_live_ingest();
        cache.end_live_ingest();
        assert_eq!(
            cache.resolve_fetch_range(location(0, 1), location(0, 2)),
            FetchRangeResolution::InvalidRange
        );
        cache.end_live_ingest();
        assert_eq!(
            cache.resolve_fetch_range(location(0, 1), location(0, 2)),
            FetchRangeResolution::NotCovered
        );
    }

    #[test]
    fn resolve_fetch_range_uses_fetch_known_range_before_live_coverage_start() {
        // Arrange: live cache started at g3, then an upstream FETCH filled g0..g2
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 3, &[0]);
        let _ = cache.insert(stream_object(0, 0), InsertOrigin::Fill);
        cache.insert_fetch_known_range(location(0, 0), location(3, 0));
        // Act / Assert
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(3, 0)),
            FetchRangeResolution::Serve {
                end_location: location(3, 0)
            }
        );
    }

    #[test]
    fn covers_live_range_from_first_ingested_object_to_open_largest_group() {
        // Arrange: live ingestion started at group 3, groups 3 and 4 closed, group 5 open
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 3, &[0]);
        insert_closed_live_group(&cache, 4, &[0]);
        let _g5 = open_live_group(&cache, 5, &[0]);
        // Act / Assert
        assert!(cache.covers(location(3, 0), location(5, 1)));
        assert!(!cache.covers(location(0, 0), location(5, 1)));
    }

    #[test]
    fn covers_merged_fetch_and_live_ranges_but_not_the_unknown_tail() {
        // Arrange: upstream FETCH filled groups 0..2, then live ingest reached group 5
        let cache = TrackCache::new();
        cache.insert_fetch_known_range(location(0, 0), location(2, 0));
        insert_closed_live_group(&cache, 3, &[0]);
        insert_closed_live_group(&cache, 4, &[0]);
        let _g5 = open_live_group(&cache, 5, &[0]);
        // Act / Assert
        assert!(cache.covers(location(0, 0), location(5, 1)));
        assert!(!cache.covers(location(0, 0), location(6, 0)));
    }

    #[tokio::test]
    async fn fetch_objects_excludes_end_object() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 1, 2, 3, 4]);
        // Act / Assert
        let objects = fetch(&cache, location(0, 0), location(0, 3)).await;
        assert_eq!(object_ids(&objects), vec![(0, 0), (0, 1), (0, 2)]);
    }

    #[tokio::test]
    async fn fetch_objects_preserves_status_objects() {
        // Arrange: a fill-only group carrying positive knowledge about non-existence
        let cache = TrackCache::new();
        let _ = cache.insert(
            status_object(0, 2, ObjectStatus::DoesNotExist),
            InsertOrigin::Fill,
        );
        // Act
        let objects = fetch(&cache, location(0, 0), location(0, 3)).await;
        // Assert
        assert_eq!(object_ids(&objects), vec![(0, 2)]);
        assert!(matches!(
            objects[0].fetch_object,
            moqt::FetchObject::Status(ObjectStatus::DoesNotExist)
        ));
    }

    #[tokio::test]
    async fn fetch_objects_end_object_zero_returns_entire_group() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 1, 2, 3, 4]);
        // Act / Assert
        let objects = fetch(&cache, location(0, 0), location(0, 0)).await;
        assert_eq!(
            object_ids(&objects),
            vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4)]
        );
    }

    #[tokio::test]
    async fn fetch_objects_filters_range_with_gaps() {
        // Arrange: gapped object_ids 0, 3, 5, 8
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 3, 5, 8]);
        // Act / Assert: start 3 inclusive, end 8 exclusive
        let objects = fetch(&cache, location(0, 3), location(0, 8)).await;
        assert_eq!(object_ids(&objects), vec![(0, 3), (0, 5)]);
    }

    #[tokio::test]
    async fn fetch_objects_spans_groups_with_exclusive_end() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 1, 2, 3, 4]);
        insert_closed_live_group(&cache, 1, &[0, 1, 2, 3, 4]);
        // Act / Assert
        let objects = fetch(&cache, location(0, 2), location(1, 3)).await;
        assert_eq!(
            object_ids(&objects),
            vec![(0, 2), (0, 3), (0, 4), (1, 0), (1, 1), (1, 2)]
        );
    }

    #[tokio::test]
    async fn fetch_objects_merges_subgroups_by_object_id() {
        // Arrange: one group split across even and odd object ids
        let cache = TrackCache::new();
        let even = open_live_group(&cache, 0, &[0, 2, 4]);
        let odd = cache.open_live_subgroup(SubgroupKey::Stream {
            group_id: 0,
            subgroup_id: 1,
        });
        for object_id in [1, 3, 5] {
            let _ = odd.insert(stream_object_in_subgroup(0, 1, object_id));
        }
        drop(even);
        drop(odd);
        // Act / Assert
        let objects = fetch(&cache, location(0, 0), location(0, 6)).await;
        assert_eq!(
            object_ids(&objects),
            vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5)]
        );
    }

    #[tokio::test]
    async fn fetch_objects_includes_datagram_objects_with_object_id_as_subgroup_id() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0]);
        let _ = cache.insert(datagram_object(0, 1), InsertOrigin::Live);
        // Act
        let objects = fetch(&cache, location(0, 0), location(0, 2)).await;
        // Assert: §10.4.4
        assert_eq!(object_ids(&objects), vec![(0, 0), (0, 1)]);
        assert_eq!(objects[1].subgroup_id, 1);
    }

    #[tokio::test]
    async fn fetch_objects_descending_groups_keep_objects_ascending() {
        // Arrange
        let cache = TrackCache::new();
        insert_closed_live_group(&cache, 0, &[0, 1]);
        insert_closed_live_group(&cache, 1, &[0, 1]);
        // Act
        let objects = cache
            .fetch_objects(location(0, 0), location(1, 2), moqt::GroupOrder::Descending)
            .await
            .unwrap();
        // Assert
        assert_eq!(object_ids(&objects), vec![(1, 0), (1, 1), (0, 0), (0, 1)]);
    }

    #[tokio::test]
    async fn fetch_objects_waits_for_in_flight_group() {
        // Arrange: group 0 is open with no objects yet while group 1 already
        // completed — the cross-stream reordering QUIC allows
        let cache = Arc::new(TrackCache::new());
        let live_g0 = cache.open_live_subgroup(stream_key(0));
        insert_closed_live_group(&cache, 1, &[0, 1, 2, 3, 4]);
        // Act: fetch [{0,0}, {1,3}) while group 0's objects have not arrived yet
        let fetch = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .fetch_objects(location(0, 0), location(1, 3), moqt::GroupOrder::Ascending)
                    .await
            }
        });
        tokio::task::yield_now().await;
        for object_id in 0..3 {
            let _ = live_g0.insert(stream_object(0, object_id));
        }
        drop(live_g0);
        // Assert: the late group 0 objects are delivered, not silently dropped
        let objects = tokio::time::timeout(Duration::from_secs(5), fetch)
            .await
            .expect("fetch must complete once the in-flight group closes")
            .unwrap()
            .unwrap();
        assert_eq!(
            object_ids(&objects),
            vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]
        );
    }

    #[tokio::test]
    async fn fetch_objects_aborts_when_track_goes_malformed() {
        // Arrange: a live group whose tail the fetch will wait for
        let cache = Arc::new(TrackCache::new());
        let live = open_live_group(&cache, 0, &[0]);
        let fetch = tokio::spawn({
            let cache = cache.clone();
            async move {
                cache
                    .fetch_objects(location(0, 0), location(0, 3), moqt::GroupOrder::Ascending)
                    .await
            }
        });
        tokio::task::yield_now().await;
        // Act: a conflicting duplicate latches the track mid-wait
        let outcome = live.insert(stream_object_in_subgroup(0, 1, 0));
        assert_eq!(outcome, Err(TrackMalformed));
        // Assert
        let result = tokio::time::timeout(Duration::from_secs(5), fetch)
            .await
            .expect("fetch must abort once the track is malformed")
            .unwrap();
        assert!(matches!(result, Err(TrackMalformed)));
    }
}
