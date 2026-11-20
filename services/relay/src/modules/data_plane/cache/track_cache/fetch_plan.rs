use std::sync::Arc;

use tokio::time::Instant;

use crate::modules::{
    data_plane::cache::{
        cached_object::CachedObject,
        known_ranges::{KnownRanges, following},
    },
    domain::object_cursor::ObjectCursor,
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
                .group_objects(group_id, ObjectCursor::At(region.first_in(group_id)))
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
            ..CacheStamp::default()
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

    mod properties {
        use std::{cmp::Reverse, collections::BTreeMap};

        use moqt::object::Location;
        use proptest::{
            collection::{btree_map, vec},
            prelude::*,
            sample::select,
        };

        use super::*;
        use crate::modules::test_support::location_strategies::{
            bound, last_held, object_location, positions, stand_in,
        };

        /// The groups the cache holds objects and known ranges in.
        const GROUPS: u64 = 6;

        /// The steps a FETCH of `start..end` (FETCH_OK End Location encoding)
        /// gets, from every position it covers in delivery order: each
        /// unexpired cached object, and one unknown stretch through the
        /// position before the next object, or the last position, for any
        /// run of positions holding an expired object or not known.
        fn expected_steps(
            objects: &BTreeMap<Location, bool>,
            known: &[(Location, Location)],
            start: Location,
            end: Location,
            descending: bool,
        ) -> Vec<Result<(u64, u64), (u64, u64)>> {
            let end = if end.object_id == 0 {
                location(end.group_id + 1, 0)
            } else {
                end
            };
            let mut covered: Vec<Location> = positions(GROUPS + 3)
                .into_iter()
                .filter(|p| start <= *p && *p < end)
                .collect();
            if descending {
                covered.sort_by_key(|p| (Reverse(p.group_id), p.object_id));
            }
            let mut steps = Vec::new();
            let mut unknown = false;
            let mut previous: Option<Location> = None;
            for at in covered {
                match objects.get(&at) {
                    Some(false) => {
                        if unknown {
                            let last =
                                last_held(previous.expect("an unknown position came before"));
                            steps.push(Err((last.group_id, last.object_id)));
                        }
                        steps.push(Ok((at.group_id, at.object_id)));
                        unknown = false;
                    }
                    Some(true) => unknown = true,
                    None => {
                        unknown |= !known.iter().any(|(from, until)| *from <= at && at < *until)
                    }
                }
                previous = Some(at);
            }
            if unknown {
                let last = last_held(previous.expect("an unknown position came before"));
                steps.push(Err((last.group_id, last.object_id)));
            }
            steps
        }

        /// A FETCH's `start` and its `end` in FETCH_OK End Location encoding,
        /// where Object ID 0 stands for the whole group.
        fn fetch_range() -> impl Strategy<Value = (Location, Location)> {
            stand_in(GROUPS).prop_flat_map(|start| {
                let ends: Vec<Location> = positions(start.group_id + 3)
                    .into_iter()
                    .filter(|end| {
                        end.group_id >= start.group_id && (end.object_id == 0 || start < *end)
                    })
                    .collect();
                (Just(start), select(ends))
            })
        }

        proptest! {
            #[test]
            fn the_plan_is_the_unexpired_objects_with_an_unknown_stretch_for_each_undecided_gap(
                objects in btree_map(object_location(GROUPS), any::<bool>(), 0..16),
                known in vec((bound(GROUPS), bound(GROUPS)), 0..5),
                (start, end) in fetch_range(),
                descending in any::<bool>(),
            ) {
                // Arrange: `objects` maps each cached object to whether it expired
                let cache = TrackCache::new();
                for (&at, &expired) in &objects {
                    let mut object = stream_object(at.group_id, at.object_id);
                    if expired {
                        object.expires_at = Some(object.received_at);
                    }
                    cache.insert(object).expect("no conflict");
                }
                for &(from, until) in &known {
                    cache.insert_known_until(from, until);
                }
                let group_order = if descending {
                    moqt::object::GroupOrder::Descending
                } else {
                    moqt::object::GroupOrder::Ascending
                };

                // Act
                let plan = cache.plan_fetch(start, end, group_order);

                // Assert
                prop_assert_eq!(
                    planned_steps(plan),
                    expected_steps(&objects, &known, start, end, descending)
                );
            }
        }
    }
}
