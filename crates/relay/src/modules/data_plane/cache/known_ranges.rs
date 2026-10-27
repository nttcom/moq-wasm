use std::ops::Range;

#[derive(Debug, Default)]
pub(crate) struct KnownRanges {
    ranges: Vec<Range<moqt::object::Location>>,
}

impl KnownRanges {
    pub(crate) fn insert(&mut self, start: moqt::object::Location, end: moqt::object::Location) {
        let end = Self::exclusive_end(end);
        if start >= end {
            return;
        }

        if let Some(last) = self.ranges.last_mut()
            && last.start <= start
            && start <= last.end
        {
            if end > last.end {
                last.end = end;
            }
            return;
        }

        let mut merged = start..end;
        let mut next_ranges = Vec::with_capacity(self.ranges.len() + 1);
        let mut inserted = false;

        for range in self.ranges.drain(..) {
            if range.end < merged.start {
                next_ranges.push(range);
            } else if merged.end < range.start {
                if !inserted {
                    next_ranges.push(merged.clone());
                    inserted = true;
                }
                next_ranges.push(range);
            } else {
                merged.start = merged.start.min(range.start);
                merged.end = merged.end.max(range.end);
            }
        }

        if !inserted {
            next_ranges.push(merged);
        }
        self.ranges = next_ranges;
    }

    /// `locations` must be ascending.
    pub(crate) fn remove_locations(
        &mut self,
        locations: impl IntoIterator<Item = moqt::object::Location>,
    ) {
        let mut cuts = locations
            .into_iter()
            .map(|at| at..Self::after(at))
            .filter(|cut| cut.start < cut.end)
            .peekable();
        let mut ranges = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            let mut start = range.start;
            while let Some(cut) = cuts.peek() {
                if cut.start >= range.end {
                    break;
                }
                if start < cut.start {
                    ranges.push(start..cut.start);
                }
                start = start.max(cut.end);
                cuts.next();
            }
            if start < range.end {
                ranges.push(start..range.end);
            }
        }
        self.ranges = ranges;
    }

    pub(crate) fn contains_range(
        &self,
        start: moqt::object::Location,
        end: moqt::object::Location,
    ) -> bool {
        let end = Self::exclusive_end(end);
        if start >= end {
            return false;
        }
        self.ranges
            .iter()
            .any(|range| range.start <= start && end <= range.end)
    }

    /// Positions below that end are fully decided: an absent object there is
    /// known-nonexistent, so readers never need to wait on them.
    pub(crate) fn end_of_range_containing(
        &self,
        location: moqt::object::Location,
    ) -> Option<moqt::object::Location> {
        self.ranges
            .iter()
            .find(|range| range.contains(&location))
            .map(|range| range.end)
    }

    fn after(at: moqt::object::Location) -> moqt::object::Location {
        moqt::object::Location {
            group_id: at.group_id,
            object_id: at.object_id.saturating_add(1),
        }
    }

    pub(crate) fn exclusive_end(location: moqt::object::Location) -> moqt::object::Location {
        if location.object_id != 0 {
            return location;
        }

        // `{group, 0}` denotes the whole group as an End Location. Internally we
        // keep half-open ranges, so this becomes the next group's first object.
        // u64::MAX groups are not practically reachable; saturating keeps ordering valid.
        moqt::object::Location {
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
    fn removing_locations_splits_only_the_ranges_holding_them() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(0, 0), location(0, 4));
        ranges.insert(location(2, 0), location(3, 0));
        // Act
        ranges.remove_locations([location(0, 0), location(0, 2), location(2, 5)]);
        // Assert
        assert!(!ranges.contains_range(location(0, 0), location(0, 1)));
        assert!(ranges.contains_range(location(0, 1), location(0, 2)));
        assert!(!ranges.contains_range(location(0, 2), location(0, 3)));
        assert!(ranges.contains_range(location(0, 3), location(0, 4)));
        assert!(ranges.contains_range(location(2, 0), location(2, 5)));
        assert!(!ranges.contains_range(location(2, 5), location(2, 6)));
        assert!(ranges.contains_range(location(2, 6), location(3, 0)));
    }
}
