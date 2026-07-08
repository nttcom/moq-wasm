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
        let end = Self::normalize_end_location(end);
        if Self::location_cmp(start, end).is_ge() {
            return;
        }

        if let Some(last) = self.ranges.last_mut()
            && Self::location_cmp(last.start, start).is_le()
            && Self::location_cmp(start, last.end).is_le()
        {
            if Self::location_cmp(end, last.end).is_gt() {
                last.end = end;
            }
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

    pub(crate) fn remove_range(&mut self, start: moqt::Location, end: moqt::Location) {
        let end = Self::normalize_end_location(end);
        if Self::location_cmp(start, end).is_ge() {
            return;
        }

        let mut ranges = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            if Self::location_cmp(range.end, start).is_le()
                || Self::location_cmp(end, range.start).is_le()
            {
                ranges.push(range);
                continue;
            }

            if Self::location_cmp(range.start, start).is_lt() {
                ranges.push(KnownRange {
                    start: range.start,
                    end: start,
                });
            }
            if Self::location_cmp(end, range.end).is_lt() {
                ranges.push(KnownRange {
                    start: end,
                    end: range.end,
                });
            }
        }
        self.ranges = ranges;
    }

    pub(crate) fn contains_range(&self, start: moqt::Location, end: moqt::Location) -> bool {
        let end = Self::normalize_end_location(end);
        if Self::location_cmp(start, end).is_ge() {
            return false;
        }
        self.ranges.iter().any(|range| {
            Self::location_cmp(range.start, start).is_le()
                && Self::location_cmp(end, range.end).is_le()
        })
    }

    fn normalize_end_location(location: moqt::Location) -> moqt::Location {
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
        assert!(ranges.contains_range(loc(3, 0), loc(3, 5)));
        assert!(ranges.contains_range(loc(1, 0), loc(2, 0)));
        assert!(!ranges.contains_range(loc(0, 0), loc(4, 0)));
    }

    #[test]
    fn whole_group_end_requires_full_group_knowledge() {
        let mut ranges = KnownRanges::default();
        ranges.insert(loc(0, 0), loc(2, 7));

        assert!(!ranges.contains_range(loc(0, 0), loc(2, 0)));
    }

    #[test]
    fn merges_overlapping_ranges() {
        let mut ranges = KnownRanges::default();
        ranges.insert(loc(0, 0), loc(2, 0));
        ranges.insert(loc(1, 0), loc(3, 0));

        assert!(ranges.contains_range(loc(0, 0), loc(3, 0)));
    }

    #[test]
    fn remove_range_can_split_existing_range() {
        let mut ranges = KnownRanges::default();
        ranges.insert(loc(0, 0), loc(5, 0));

        ranges.remove_range(loc(2, 0), loc(3, 0));

        assert!(ranges.contains_range(loc(0, 0), loc(1, 0)));
        assert!(!ranges.contains_range(loc(2, 0), loc(3, 0)));
        assert!(ranges.contains_range(loc(4, 0), loc(5, 0)));
    }
}
