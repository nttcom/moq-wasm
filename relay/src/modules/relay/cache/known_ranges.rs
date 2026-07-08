use std::cmp::Ordering;

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
        if Self::location_cmp(start, end).is_ge() {
            return;
        }

        let mut merged = KnownRange { start, end };
        let mut next_ranges = Vec::with_capacity(self.ranges.len() + 1);
        let mut inserted = false;

        for range in self.ranges.drain(..) {
            if Self::location_cmp(range.end, merged.start).is_lt() {
                next_ranges.push(range);
            } else if Self::location_cmp(merged.end, range.start).is_lt() {
                if !inserted {
                    next_ranges.push(merged);
                    inserted = true;
                }
                next_ranges.push(range);
            } else {
                merged.start = Self::min_location(merged.start, range.start);
                merged.end = Self::max_location(merged.end, range.end);
            }
        }

        if !inserted {
            next_ranges.push(merged);
        }
        self.ranges = next_ranges;
    }

    pub(crate) fn remove_up_to(&mut self, location: moqt::Location) {
        let mut ranges = Vec::with_capacity(self.ranges.len());
        for mut range in self.ranges.drain(..) {
            if Self::location_cmp(range.end, location).is_le() {
                continue;
            }
            if Self::location_cmp(range.start, location).is_lt() {
                range.start = location;
            }
            ranges.push(range);
        }
        self.ranges = ranges;
    }

    pub(crate) fn contains_range(&self, start: moqt::Location, end: moqt::Location) -> bool {
        if Self::location_cmp(start, end).is_ge() {
            return false;
        }
        self.ranges.iter().any(|range| {
            Self::location_cmp(range.start, start).is_le()
                && Self::location_cmp(end, range.end).is_le()
        })
    }

    fn min_location(left: moqt::Location, right: moqt::Location) -> moqt::Location {
        if Self::location_cmp(left, right).is_le() {
            left
        } else {
            right
        }
    }

    fn max_location(left: moqt::Location, right: moqt::Location) -> moqt::Location {
        if Self::location_cmp(left, right).is_ge() {
            left
        } else {
            right
        }
    }

    fn location_cmp(left: moqt::Location, right: moqt::Location) -> Ordering {
        (left.group_id, left.object_id).cmp(&(right.group_id, right.object_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(group_id: u64, object_id: u64) -> moqt::Location {
        moqt::Location {
            group_id,
            object_id,
        }
    }

    #[test]
    fn contains_inserted_range() {
        let mut ranges = KnownRanges::default();
        ranges.insert(loc(0, 0), loc(3, 0));

        assert!(ranges.contains_range(loc(0, 0), loc(3, 0)));
        assert!(ranges.contains_range(loc(1, 0), loc(2, 0)));
        assert!(!ranges.contains_range(loc(0, 0), loc(4, 0)));
    }

    #[test]
    fn merges_overlapping_ranges() {
        let mut ranges = KnownRanges::default();
        ranges.insert(loc(0, 0), loc(2, 0));
        ranges.insert(loc(1, 0), loc(3, 0));

        assert!(ranges.contains_range(loc(0, 0), loc(3, 0)));
    }

    #[test]
    fn remove_up_to_trims_ranges() {
        let mut ranges = KnownRanges::default();
        ranges.insert(loc(0, 0), loc(3, 0));
        ranges.remove_up_to(loc(1, 0));

        assert!(!ranges.contains_range(loc(0, 0), loc(1, 0)));
        assert!(ranges.contains_range(loc(1, 0), loc(3, 0)));
    }
}
