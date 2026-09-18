use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use bytes::Bytes;
use moqt::{ExtensionHeaders, Location};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedObject {
    pub(crate) location: Location,
    pub(crate) extension_headers: ExtensionHeaders,
    pub(crate) payload: Bytes,
}

struct Entry {
    object: CachedObject,
    cached_at: Instant,
}

/// Objects the publisher can still replay for a FETCH (draft-ietf-moq-transport-14
/// §9.16), kept for `retention` after they were produced.
pub(crate) struct ObjectCache {
    retention: Duration,
    entries: VecDeque<Entry>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FetchRange {
    Serve {
        objects: Vec<CachedObject>,
        end_location: Location,
    },
    InvalidRange,
    NoObjects,
}

impl ObjectCache {
    pub(crate) fn new(retention: Duration) -> Self {
        Self {
            retention,
            entries: VecDeque::new(),
        }
    }

    pub(crate) fn insert(&mut self, object: CachedObject) {
        self.insert_at(object, Instant::now());
    }

    pub(crate) fn insert_at(&mut self, object: CachedObject, now: Instant) {
        self.entries.push_back(Entry {
            object,
            cached_at: now,
        });
        while self
            .entries
            .front()
            .is_some_and(|entry| now.duration_since(entry.cached_at) > self.retention)
        {
            self.entries.pop_front();
        }
    }

    /// `end` is exclusive, except that an object id of 0 requests the whole
    /// group (§9.16.1). The returned end location follows §9.17: the location
    /// after the largest object when the request reaches past it, the whole
    /// group when a closed group was requested in full, the request otherwise.
    pub(crate) fn resolve(&self, start: Location, end: Location) -> FetchRange {
        if explicit_end_before_or_equal_start(start, end) {
            return FetchRange::InvalidRange;
        }
        let Some(largest) = self.entries.back().map(|entry| entry.object.location) else {
            return FetchRange::NoObjects;
        };
        if start > largest {
            return FetchRange::InvalidRange;
        }
        let objects: Vec<CachedObject> = self
            .entries
            .iter()
            .map(|entry| &entry.object)
            .filter(|object| object.location >= start && before_end(object.location, end))
            .cloned()
            .collect();
        if objects.is_empty() {
            return FetchRange::NoObjects;
        }
        FetchRange::Serve {
            objects,
            end_location: response_end_location(end, largest),
        }
    }
}

fn explicit_end_before_or_equal_start(start: Location, end: Location) -> bool {
    start.group_id > end.group_id
        || (start.group_id == end.group_id
            && end.object_id != 0
            && start.object_id >= end.object_id)
}

fn before_end(location: Location, end: Location) -> bool {
    location.group_id < end.group_id
        || (location.group_id == end.group_id
            && (end.object_id == 0 || location.object_id < end.object_id))
}

fn response_end_location(requested_end: Location, largest: Location) -> Location {
    let after_largest = Location {
        group_id: largest.group_id,
        object_id: largest.object_id + 1,
    };
    if requested_end.group_id > largest.group_id {
        return after_largest;
    }
    if requested_end.object_id == 0 {
        return if requested_end.group_id < largest.group_id {
            requested_end
        } else {
            after_largest
        };
    }
    if requested_end.group_id == largest.group_id
        && requested_end.object_id > after_largest.object_id
    {
        return after_largest;
    }
    requested_end
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETENTION: Duration = Duration::from_secs(30);

    fn location(group_id: u64, object_id: u64) -> Location {
        Location {
            group_id,
            object_id,
        }
    }

    fn object(group_id: u64, object_id: u64) -> CachedObject {
        CachedObject {
            location: location(group_id, object_id),
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from(format!("g{group_id}o{object_id}")),
        }
    }

    fn cache_with_two_closed_groups_and_an_open_one() -> ObjectCache {
        let mut cache = ObjectCache::new(RETENTION);
        for (group_id, object_id) in [(7, 0), (7, 1), (8, 0), (8, 1), (8, 2), (9, 0)] {
            cache.insert(object(group_id, object_id));
        }
        cache
    }

    fn locations(range: FetchRange) -> (Vec<(u64, u64)>, Location) {
        match range {
            FetchRange::Serve {
                objects,
                end_location,
            } => (
                objects
                    .iter()
                    .map(|object| (object.location.group_id, object.location.object_id))
                    .collect(),
                end_location,
            ),
            other => panic!("expected a served range, got {other:?}"),
        }
    }

    #[test]
    fn serves_a_whole_closed_group_when_the_end_object_is_zero() {
        // Arrange
        let cache = cache_with_two_closed_groups_and_an_open_one();

        // Act
        let (objects, end_location) = locations(cache.resolve(location(8, 0), location(8, 0)));

        // Assert
        assert_eq!(objects, [(8, 0), (8, 1), (8, 2)]);
        assert_eq!(end_location, location(8, 0));
    }

    #[test]
    fn serves_an_explicit_range_across_groups_with_an_exclusive_end() {
        // Arrange
        let cache = cache_with_two_closed_groups_and_an_open_one();

        // Act
        let (objects, end_location) = locations(cache.resolve(location(7, 1), location(8, 2)));

        // Assert
        assert_eq!(objects, [(7, 1), (8, 0), (8, 1)]);
        assert_eq!(end_location, location(8, 2));
    }

    #[test]
    fn ends_after_the_largest_object_when_the_request_reaches_the_open_group() {
        // Arrange
        let cache = cache_with_two_closed_groups_and_an_open_one();

        // Act
        let (whole_open_group, open_end) = locations(cache.resolve(location(9, 0), location(9, 0)));
        let (beyond, beyond_end) = locations(cache.resolve(location(8, 2), location(20, 5)));

        // Assert
        assert_eq!(whole_open_group, [(9, 0)]);
        assert_eq!(open_end, location(9, 1));
        assert_eq!(beyond, [(8, 2), (9, 0)]);
        assert_eq!(beyond_end, location(9, 1));
    }

    #[test]
    fn rejects_ranges_that_end_before_they_start_or_start_past_the_largest_object() {
        // Arrange
        let cache = cache_with_two_closed_groups_and_an_open_one();

        // Act / Assert
        assert_eq!(
            cache.resolve(location(8, 2), location(8, 1)),
            FetchRange::InvalidRange
        );
        assert_eq!(
            cache.resolve(location(9, 0), location(8, 0)),
            FetchRange::InvalidRange
        );
        assert_eq!(
            cache.resolve(location(9, 1), location(10, 0)),
            FetchRange::InvalidRange
        );
    }

    #[test]
    fn reports_no_objects_for_an_empty_cache_or_an_evicted_range() {
        // Arrange
        let mut cache = ObjectCache::new(RETENTION);
        let start = Instant::now();
        let empty = cache.resolve(location(0, 0), location(1, 0));
        cache.insert_at(object(1, 0), start);
        cache.insert_at(object(2, 0), start + RETENTION + Duration::from_secs(1));

        // Act
        let evicted = cache.resolve(location(1, 0), location(1, 0));
        let (kept, _) = locations(cache.resolve(location(1, 0), location(3, 0)));

        // Assert
        assert_eq!(empty, FetchRange::NoObjects);
        assert_eq!(evicted, FetchRange::NoObjects);
        assert_eq!(kept, [(2, 0)]);
    }
}
