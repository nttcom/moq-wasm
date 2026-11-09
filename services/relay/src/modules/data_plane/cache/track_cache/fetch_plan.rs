use std::sync::Arc;

use tokio::time::Instant;

use crate::modules::data_plane::cache::{
    cached_object::CachedObject,
    known_ranges::{KnownRanges, following},
};

use super::{TrackCache, location};

/// One step of a FETCH response that waits for nothing.
#[derive(Debug, Clone)]
pub(crate) enum PlannedFetch {
    Object(Arc<CachedObject>),
    /// The positions after the previous step through `last` are not known,
    /// so their objects may exist (draft-18 §11.4.4.2).
    Unknown {
        last: moqt::object::Location,
    },
}

impl TrackCache {
    /// The response to a FETCH of `start..end` (FETCH_OK End Location
    /// encoding) from what the cache holds now: its objects in
    /// `group_order`, each stretch before, between and after them the cache
    /// does not know reported as unknown. An expired object is not sent and
    /// its position is unknown again (draft-14 §9.2.1.3).
    pub(crate) fn plan_fetch(
        &self,
        start: moqt::object::Location,
        end: moqt::object::Location,
        group_order: moqt::object::GroupOrder,
    ) -> Vec<PlannedFetch> {
        let region = Region {
            start,
            end: KnownRanges::exclusive_end(end),
            descending: matches!(group_order, moqt::object::GroupOrder::Descending),
        };
        let ledger = self.read();
        let now = Instant::now();
        let mut groups = ledger.groups_in_range(start.group_id, end.group_id);
        if region.descending {
            groups.reverse();
        }
        let mut plan = Vec::new();
        let mut prev = None;
        let mut expired_in_gap = false;
        for group_id in groups {
            for object in ledger
                .group_objects(group_id, region.first_in(group_id))
                .take_while(|object| object.object.location < region.end)
            {
                if object.is_expired(now) {
                    expired_in_gap = true;
                    continue;
                }
                let at = object.object.location;
                if expired_in_gap || !region.gap_known(&ledger.known_ranges, prev, Some(at)) {
                    plan.push(PlannedFetch::Unknown {
                        last: region.last_before(at),
                    });
                }
                plan.push(PlannedFetch::Object(object.clone()));
                prev = Some(at);
                expired_in_gap = false;
            }
        }
        if expired_in_gap || !region.gap_known(&ledger.known_ranges, prev, None) {
            plan.push(PlannedFetch::Unknown {
                last: region.last(),
            });
        }
        plan
    }
}

/// The positions a FETCH response covers, `end` exclusive.
struct Region {
    start: moqt::object::Location,
    end: moqt::object::Location,
    descending: bool,
}

impl Region {
    fn first_in(&self, group_id: u64) -> u64 {
        if group_id == self.start.group_id {
            self.start.object_id
        } else {
            0
        }
    }

    fn last_in(&self, group_id: u64) -> moqt::object::Location {
        if group_id == self.end.group_id {
            location(group_id, self.end.object_id - 1)
        } else {
            location(group_id, u64::MAX)
        }
    }

    /// The last position the response covers.
    fn last(&self) -> moqt::object::Location {
        match (self.descending, self.end.object_id) {
            (true, _) => self.last_in(self.start.group_id),
            (false, 0) => self.last_in(self.end.group_id - 1),
            (false, _) => self.last_in(self.end.group_id),
        }
    }

    /// The position delivered right before `next`.
    fn last_before(&self, next: moqt::object::Location) -> moqt::object::Location {
        if next.object_id > self.first_in(next.group_id) {
            return location(next.group_id, next.object_id - 1);
        }
        if self.descending {
            self.last_in(next.group_id + 1)
        } else {
            self.last_in(next.group_id - 1)
        }
    }

    /// Whether every position delivered between `prev` (or the start of the
    /// response) and `next` (or its end) is known.
    fn gap_known(
        &self,
        known: &KnownRanges,
        prev: Option<moqt::object::Location>,
        next: Option<moqt::object::Location>,
    ) -> bool {
        if !self.descending {
            return known
                .covers_until(prev.map_or(self.start, following), next.unwrap_or(self.end));
        }
        // Descending: the rest of prev's group, then the groups below it,
        // then next's group up to next.
        if let (Some(prev), Some(next)) = (prev, next)
            && prev.group_id == next.group_id
        {
            return known.covers_until(following(prev), next);
        }
        let rest_of_prev_group = prev.is_none_or(|prev| {
            known.covers_until(
                following(prev),
                self.end.min(location(prev.group_id + 1, 0)),
            )
        });
        let below = prev.map_or(self.end, |prev| location(prev.group_id, 0));
        let lower_groups = match next {
            Some(next) => {
                known.covers_until(location(next.group_id + 1, 0), below)
                    && known
                        .covers_until(location(next.group_id, self.first_in(next.group_id)), next)
            }
            None => known.covers_until(self.start, below),
        };
        rest_of_prev_group && lower_groups
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::modules::{
        data_plane::cache::cached_object::CacheStamp,
        test_support::relay_harness::fixtures::{
            cached_object::{insert_closed_group, stream_object},
            planned_steps,
        },
    };

    /// Groups 0 and 2 closed with objects 0 and 1; group 1 was never seen.
    fn cache_missing_group_1() -> TrackCache {
        let cache = TrackCache::new();
        insert_closed_group(&cache, 0, &[0, 1]);
        insert_closed_group(&cache, 2, &[0, 1]);
        cache
    }

    #[test]
    fn a_known_range_is_planned_as_its_objects_alone() {
        // Arrange
        let cache = cache_missing_group_1();

        // Act
        let plan = cache.plan_fetch(
            location(2, 0),
            location(2, 0),
            moqt::object::GroupOrder::Ascending,
        );

        // Assert
        assert_eq!(planned_steps(plan), vec![Ok((2, 0)), Ok((2, 1))]);
    }

    #[test]
    fn an_unseen_group_is_planned_as_unknown_in_either_order() {
        // Arrange
        let cache = cache_missing_group_1();

        // Act
        let ascending = cache.plan_fetch(
            location(0, 0),
            location(2, 0),
            moqt::object::GroupOrder::Ascending,
        );
        let descending = cache.plan_fetch(
            location(0, 0),
            location(2, 0),
            moqt::object::GroupOrder::Descending,
        );

        // Assert: draft-18 §11.4.4.2, the unknown stretch ends where the
        // next object's group begins
        assert_eq!(
            planned_steps(ascending),
            vec![
                Ok((0, 0)),
                Ok((0, 1)),
                Err((1, u64::MAX)),
                Ok((2, 0)),
                Ok((2, 1))
            ]
        );
        assert_eq!(
            planned_steps(descending),
            vec![
                Ok((2, 0)),
                Ok((2, 1)),
                Err((1, u64::MAX)),
                Ok((0, 0)),
                Ok((0, 1))
            ]
        );
    }

    #[test]
    fn a_range_beyond_the_cached_objects_ends_with_an_unknown_stretch() {
        // Arrange
        let cache = cache_missing_group_1();

        // Act
        let plan = cache.plan_fetch(
            location(2, 0),
            location(4, 3),
            moqt::object::GroupOrder::Ascending,
        );

        // Assert
        assert_eq!(
            planned_steps(plan),
            vec![Ok((2, 0)), Ok((2, 1)), Err((4, 2))]
        );
    }

    #[test]
    fn a_range_the_cache_knows_nothing_of_is_one_unknown_stretch() {
        // Arrange
        let cache = TrackCache::new();

        // Act
        let plan = cache.plan_fetch(
            location(5, 2),
            location(7, 0),
            moqt::object::GroupOrder::Ascending,
        );

        // Assert
        assert_eq!(planned_steps(plan), vec![Err((7, u64::MAX))]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_object_is_planned_as_unknown() {
        // Arrange: a fill of {0,0}..{0,3} whose object {0,1} expires first
        let cache = TrackCache::new();
        let expiring = CacheStamp {
            max_cache_duration: Some(Duration::from_millis(10)),
            persist: false,
        };
        for object in [
            stream_object(0, 0),
            expiring.apply(stream_object(0, 1)),
            stream_object(0, 2),
        ] {
            cache.insert(object).expect("no conflict");
        }
        cache.insert_fetch_known_range(location(0, 0), location(0, 3));
        tokio::time::advance(Duration::from_millis(20)).await;

        // Act
        let plan = cache.plan_fetch(
            location(0, 0),
            location(0, 3),
            moqt::object::GroupOrder::Ascending,
        );

        // Assert: draft-14 §9.2.1.3
        assert_eq!(
            planned_steps(plan),
            vec![Ok((0, 0)), Err((0, 1)), Ok((0, 2))]
        );
    }
}
