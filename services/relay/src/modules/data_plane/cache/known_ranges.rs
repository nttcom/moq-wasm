use std::{collections::BTreeMap, ops::Bound};

use moqt::object::Location;

use crate::modules::domain::location_cursor::LocationCursor;

/// Half-open ranges of known positions: key = start, value = exclusive end.
/// Ranges are disjoint and never adjacent, so a contiguous known span is a
/// single entry and only the last entry starting at or before a position can
/// hold it.
#[derive(Debug, Default)]
pub(crate) struct KnownRanges {
    ranges: BTreeMap<Location, LocationCursor>,
}

impl KnownRanges {
    /// `insert_until` for a FETCH range with End Location `end`.
    pub(crate) fn insert(&mut self, start: Location, end: Location) {
        self.insert_until(LocationCursor::At(start), LocationCursor::fetch_end(end));
    }

    pub(crate) fn insert_until(&mut self, start: LocationCursor, mut end: LocationCursor) {
        let LocationCursor::At(mut start) = start else {
            return;
        };
        if LocationCursor::At(start) >= end {
            return;
        }

        if let Some((&prev_start, &prev_end)) = self.ranges.range(..=start).next_back()
            && LocationCursor::At(start) <= prev_end
        {
            start = prev_start;
        }
        // A range starting at `end` is adjacent, so it merges too.
        let through_end = match end {
            LocationCursor::At(end) => Bound::Included(end),
            LocationCursor::PastLast => Bound::Unbounded,
        };
        if let Some((_, last_end)) = self
            .ranges
            .extract_if((Bound::Included(start), through_end), |_, _| true)
            .last()
        {
            end = end.max(last_end);
        }
        self.ranges.insert(start, end);
    }

    pub(crate) fn remove_locations(&mut self, locations: &[Location]) {
        for &at in locations {
            let Some((&range_start, &range_end)) = self.ranges.range(..=at).next_back() else {
                continue;
            };
            if range_end <= LocationCursor::At(at) {
                continue;
            }
            self.ranges.remove(&range_start);
            if range_start < at {
                self.ranges.insert(range_start, LocationCursor::At(at));
            }
            if let LocationCursor::At(cut_end) = LocationCursor::after(at)
                && LocationCursor::At(cut_end) < range_end
            {
                self.ranges.insert(cut_end, range_end);
            }
        }
    }

    /// Whether every position of the FETCH range `start` to End Location
    /// `end` is known; an empty range is not.
    pub(crate) fn contains_range(&self, start: Location, end: Location) -> bool {
        let end = LocationCursor::fetch_end(end);
        LocationCursor::At(start) < end && self.covers_until(LocationCursor::At(start), end)
    }

    /// Whether every position of the half-open range `start..end` is known;
    /// an empty range is.
    pub(crate) fn covers_until(&self, start: LocationCursor, end: LocationCursor) -> bool {
        let LocationCursor::At(start) = start else {
            return true;
        };
        LocationCursor::At(start) >= end
            || self
                .end_of_range_containing(start)
                .is_some_and(|range_end| end <= range_end)
    }

    /// Positions below that end are fully decided: an absent object there is
    /// known-nonexistent, so readers never need to wait on them.
    pub(crate) fn end_of_range_containing(&self, location: Location) -> Option<LocationCursor> {
        let (_, &range_end) = self.ranges.range(..=location).next_back()?;
        (LocationCursor::At(location) < range_end).then_some(range_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::location;

    #[test]
    fn contains_inserted_range() {
        // Arrange
        let mut ranges = KnownRanges::default();
        // Act
        ranges.insert(location(0, 0), location(3, 0));
        // Assert
        assert!(ranges.contains_range(location(0, 0), location(3, 0)));
        assert!(ranges.contains_range(location(3, 0), location(3, 5)));
        assert!(ranges.contains_range(location(1, 0), location(2, 0)));
        assert!(!ranges.contains_range(location(0, 0), location(4, 0)));
    }

    #[test]
    fn whole_group_end_requires_full_group_knowledge() {
        // Arrange
        let mut ranges = KnownRanges::default();
        // Act
        ranges.insert(location(0, 0), location(2, 7));
        // Assert
        assert!(!ranges.contains_range(location(0, 0), location(2, 0)));
    }

    #[test]
    fn merges_overlapping_ranges() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(0, 0), location(2, 0));
        // Act
        ranges.insert(location(1, 0), location(3, 0));
        // Assert
        assert!(ranges.contains_range(location(0, 0), location(3, 0)));
    }

    #[test]
    fn filling_a_gap_merges_the_adjacent_ranges_on_both_sides() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(0, 0), location(0, 2));
        ranges.insert(location(0, 3), location(0, 5));
        ranges.insert(location(0, 6), location(0, 8));
        // Act
        ranges.insert(location(0, 2), location(0, 6));
        // Assert
        assert!(ranges.contains_range(location(0, 0), location(0, 8)));
        assert_eq!(
            ranges.end_of_range_containing(location(0, 0)),
            Some(LocationCursor::At(location(0, 8)))
        );
    }

    #[test]
    fn removing_locations_splits_only_the_ranges_holding_them() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(0, 0), location(0, 4));
        ranges.insert(location(2, 0), location(3, 0));
        // Act
        ranges.remove_locations(&[location(0, 0), location(0, 2), location(2, 5)]);
        // Assert
        assert!(!ranges.contains_range(location(0, 0), location(0, 1)));
        assert!(ranges.contains_range(location(0, 1), location(0, 2)));
        assert!(!ranges.contains_range(location(0, 2), location(0, 3)));
        assert!(ranges.contains_range(location(0, 3), location(0, 4)));
        assert!(ranges.contains_range(location(2, 0), location(2, 5)));
        assert!(!ranges.contains_range(location(2, 5), location(2, 6)));
        assert!(ranges.contains_range(location(2, 6), location(3, 0)));
    }

    #[test]
    fn removing_a_groups_last_object_id_forgets_only_that_position() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(0, 0), location(0, 0));
        // Act
        ranges.remove_locations(&[location(0, u64::MAX)]);
        // Assert
        assert_eq!(ranges.end_of_range_containing(location(0, u64::MAX)), None);
        assert!(ranges.covers_until(
            LocationCursor::At(location(0, 0)),
            LocationCursor::At(location(0, u64::MAX))
        ));
    }

    #[test]
    fn a_range_through_the_last_group_ends_past_the_last_location() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(u64::MAX, 0), location(u64::MAX, 0));
        // Act
        ranges.insert(location(u64::MAX - 1, 0), location(u64::MAX - 1, 0));
        // Assert
        assert!(ranges.contains_range(location(u64::MAX - 1, 0), location(u64::MAX, 0)));
        assert_eq!(
            ranges.end_of_range_containing(location(u64::MAX - 1, 3)),
            Some(LocationCursor::PastLast)
        );
    }

    #[test]
    fn removing_the_last_location_keeps_the_rest_of_its_group() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(u64::MAX, 0), location(u64::MAX, 0));
        // Act
        ranges.remove_locations(&[moqt::object::Location::MAX]);
        // Assert
        assert_eq!(
            ranges.end_of_range_containing(location(u64::MAX, 0)),
            Some(LocationCursor::At(moqt::object::Location::MAX))
        );
        assert_eq!(
            ranges.end_of_range_containing(moqt::object::Location::MAX),
            None
        );
    }

    #[test]
    fn sparse_object_ids_each_keep_their_own_range() {
        // Arrange
        let mut ranges = KnownRanges::default();
        let object_count = 50_000;
        // Act
        for object_id in 0..object_count {
            let at = location(0, object_id * 2);
            ranges.insert_until(LocationCursor::At(at), LocationCursor::after(at));
        }
        // Assert
        for object_id in 0..object_count {
            let at = location(0, object_id * 2);
            let gap = location(0, object_id * 2 + 1);
            assert!(ranges.contains_range(at, gap));
            assert!(!ranges.contains_range(gap, location(0, object_id * 2 + 2)));
            assert_eq!(
                ranges.end_of_range_containing(at),
                Some(LocationCursor::At(gap))
            );
            assert_eq!(ranges.end_of_range_containing(gap), None);
        }
    }

    mod properties {
        use std::collections::BTreeSet;

        use proptest::{collection::vec, prelude::*};

        use super::*;
        use crate::modules::test_support::location_strategies::{Groups, groups};

        const GROUPS: u64 = 4;

        #[derive(Debug, Clone)]
        enum Operation {
            InsertUntil(LocationCursor, LocationCursor),
            RemoveLocations(Vec<Location>),
        }

        fn operation(groups: Groups) -> impl Strategy<Value = Operation> {
            prop_oneof![
                (groups.bound(), groups.bound())
                    .prop_map(|(start, end)| Operation::InsertUntil(start, end)),
                vec(groups.object_location(), 0..4).prop_map(Operation::RemoveLocations),
            ]
        }

        fn groups_and_operations() -> impl Strategy<Value = (Groups, Vec<Operation>)> {
            groups(GROUPS).prop_flat_map(|groups| (Just(groups), vec(operation(groups), 1..12)))
        }

        proptest! {
            #[test]
            fn queries_match_a_set_of_known_positions((groups, operations) in groups_and_operations()) {
                // Arrange
                let mut ranges = KnownRanges::default();
                let mut model = BTreeSet::new();

                // Act
                for operation in operations {
                    match operation {
                        Operation::InsertUntil(start, end) => {
                            ranges.insert_until(start, end);
                            model.extend(groups.positions().into_iter().filter(|p| (start..end).contains(&LocationCursor::At(*p))));
                        }
                        Operation::RemoveLocations(locations) => {
                            ranges.remove_locations(&locations);
                            for at in locations {
                                model.remove(&at);
                            }
                        }
                    }
                }

                // Assert: the ranges are disjoint, non-empty and never adjacent
                for ((start, end), (next_start, _)) in ranges.ranges.iter().zip(ranges.ranges.iter().skip(1)) {
                    prop_assert!(LocationCursor::At(*start) < *end);
                    prop_assert!(*end < LocationCursor::At(*next_start));
                }
                for start in groups.bounds() {
                    for end in groups.bounds() {
                        let covered = groups.positions().iter().filter(|p| (start..end).contains(&LocationCursor::At(**p))).all(|p| model.contains(p));
                        prop_assert_eq!(ranges.covers_until(start, end), covered, "{:?}..{:?}", start, end);
                    }
                }
                for at in groups.positions() {
                    let range_end = model.contains(&at).then(|| {
                        groups.bounds()
                            .into_iter()
                            .find(|p| *p > LocationCursor::At(at) && !matches!(p, LocationCursor::At(p) if model.contains(p)))
                            .expect("the last bound is never known")
                    });
                    prop_assert_eq!(ranges.end_of_range_containing(at), range_end, "{:?}", at);
                }
            }
        }
    }
}
