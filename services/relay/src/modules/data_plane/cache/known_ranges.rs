use std::collections::BTreeMap;

use moqt::object::Location;

/// Half-open ranges of known positions: key = start, value = exclusive end.
/// Ranges are disjoint and never adjacent, so a contiguous known span is a
/// single entry and only the last entry starting at or before a position can
/// hold it.
#[derive(Debug, Default)]
pub(crate) struct KnownRanges {
    ranges: BTreeMap<Location, Location>,
}

impl KnownRanges {
    pub(crate) fn insert(&mut self, mut start: Location, end: Location) {
        let mut end = Self::exclusive_end(end);
        if start >= end {
            return;
        }

        if let Some((&prev_start, &prev_end)) = self.ranges.range(..=start).next_back()
            && start <= prev_end
        {
            start = prev_start;
        }
        if let Some((_, last_end)) = self.ranges.extract_if(start..=end, |_, _| true).last() {
            end = end.max(last_end);
        }
        self.ranges.insert(start, end);
    }

    pub(crate) fn remove_locations(&mut self, locations: &[Location]) {
        for &at in locations {
            let cut_end = Self::after(at);
            let Some((&range_start, &range_end)) = self.ranges.range(..=at).next_back() else {
                continue;
            };
            if at >= cut_end || range_end <= at {
                continue;
            }
            self.ranges.remove(&range_start);
            if range_start < at {
                self.ranges.insert(range_start, at);
            }
            if cut_end < range_end {
                self.ranges.insert(cut_end, range_end);
            }
        }
    }

    pub(crate) fn contains_range(&self, start: Location, end: Location) -> bool {
        let end = Self::exclusive_end(end);
        if start >= end {
            return false;
        }
        self.end_of_range_containing(start)
            .is_some_and(|range_end| end <= range_end)
    }

    /// Positions below that end are fully decided: an absent object there is
    /// known-nonexistent, so readers never need to wait on them.
    pub(crate) fn end_of_range_containing(&self, location: Location) -> Option<Location> {
        let (_, &range_end) = self.ranges.range(..=location).next_back()?;
        (location < range_end).then_some(range_end)
    }

    fn after(at: Location) -> Location {
        Location {
            group_id: at.group_id,
            object_id: at.object_id.saturating_add(1),
        }
    }

    pub(crate) fn exclusive_end(location: Location) -> Location {
        if location.object_id != 0 {
            return location;
        }

        // `{group, 0}` denotes the whole group as an End Location. Internally we
        // keep half-open ranges, so this becomes the next group's first object.
        // u64::MAX groups are not practically reachable; saturating keeps ordering valid.
        Location {
            group_id: location.group_id.saturating_add(1),
            object_id: 0,
        }
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
            Some(location(0, 8))
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
    fn sparse_object_ids_each_keep_their_own_range() {
        // Arrange
        let mut ranges = KnownRanges::default();
        let object_count = 50_000;
        // Act
        for object_id in 0..object_count {
            let at = location(0, object_id * 2);
            ranges.insert(at, KnownRanges::after(at));
        }
        // Assert
        for object_id in 0..object_count {
            let at = location(0, object_id * 2);
            let gap = location(0, object_id * 2 + 1);
            assert!(ranges.contains_range(at, gap));
            assert!(!ranges.contains_range(gap, KnownRanges::after(gap)));
            assert_eq!(ranges.end_of_range_containing(at), Some(gap));
            assert_eq!(ranges.end_of_range_containing(gap), None);
        }
    }
}
