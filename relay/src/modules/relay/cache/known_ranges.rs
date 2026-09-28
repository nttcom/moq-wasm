#[derive(Clone, Copy, Debug, PartialEq)]
struct KnownRange {
    start: moqt::Location,
    end: moqt::Location,
}

#[derive(Debug, Default)]
pub(crate) struct KnownRanges {
    ranges: Vec<KnownRange>,
}

impl KnownRanges {
    pub(crate) fn insert(&mut self, start: moqt::Location, end: moqt::Location) {
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

        let mut merged = KnownRange { start, end };
        let mut next_ranges = Vec::with_capacity(self.ranges.len() + 1);
        let mut inserted = false;

        for range in self.ranges.drain(..) {
            if range.end < merged.start {
                next_ranges.push(range);
            } else if merged.end < range.start {
                if !inserted {
                    next_ranges.push(merged);
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

    pub(crate) fn remove_range(&mut self, start: moqt::Location, end: moqt::Location) {
        let end = Self::exclusive_end(end);
        if start >= end {
            return;
        }

        let mut ranges = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            if range.end <= start || end <= range.start {
                ranges.push(range);
                continue;
            }

            if range.start < start {
                ranges.push(KnownRange {
                    start: range.start,
                    end: start,
                });
            }
            if end < range.end {
                ranges.push(KnownRange {
                    start: end,
                    end: range.end,
                });
            }
        }
        self.ranges = ranges;
    }

    pub(crate) fn contains_range(&self, start: moqt::Location, end: moqt::Location) -> bool {
        let end = Self::exclusive_end(end);
        if start >= end {
            return false;
        }
        self.ranges
            .iter()
            .any(|range| range.start <= start && end <= range.end)
    }

    /// Returns the exclusive end of the range containing `location`, if any.
    /// Positions below that end are fully decided: an absent object there is
    /// known-nonexistent, so readers never need to wait on them.
    pub(crate) fn end_of_range_containing(
        &self,
        location: moqt::Location,
    ) -> Option<moqt::Location> {
        self.ranges
            .iter()
            .find(|range| range.start <= location && location < range.end)
            .map(|range| range.end)
    }

    pub(crate) fn exclusive_end(location: moqt::Location) -> moqt::Location {
        if location.object_id != 0 {
            return location;
        }

        // `{group, 0}` denotes the whole group as an End Location. Internally we
        // keep half-open ranges, so this becomes the next group's first object.
        // u64::MAX groups are not practically reachable; saturating keeps ordering valid.
        moqt::Location {
            group_id: location.group_id.saturating_add(1),
            object_id: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::relay::tests::harness::fixtures::location;

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
    fn remove_range_can_split_existing_range() {
        // Arrange
        let mut ranges = KnownRanges::default();
        ranges.insert(location(0, 0), location(5, 0));
        // Act
        ranges.remove_range(location(2, 0), location(3, 0));
        // Assert
        assert!(ranges.contains_range(location(0, 0), location(1, 0)));
        assert!(!ranges.contains_range(location(2, 0), location(3, 0)));
        assert!(ranges.contains_range(location(4, 0), location(5, 0)));
    }
}
