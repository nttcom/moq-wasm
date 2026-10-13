use std::{collections::VecDeque, time::Duration};

use bytes::Bytes;

use crate::{
    modules::moqt::domains::fetch_range::{FetchRangeError, resolve_fetch_range},
    object::{ExtensionHeaders, Location},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CachedObject {
    pub(super) location: Location,
    pub(super) extension_headers: ExtensionHeaders,
    pub(super) payload: Bytes,
}

struct Entry {
    object: CachedObject,
    cached_at: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    For(Duration),
    NewestOnly,
}

pub(super) struct ObjectCache {
    retention: Retention,
    entries: VecDeque<Entry>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FetchRange {
    Serve {
        objects: Vec<CachedObject>,
        end_location: Location,
    },
    InvalidRange,
    NoObjects,
}

impl ObjectCache {
    pub(super) fn new(retention: Retention) -> Self {
        Self {
            retention,
            entries: VecDeque::new(),
        }
    }

    pub(super) fn insert(&mut self, object: CachedObject, now: Duration) {
        self.entries.push_back(Entry {
            object,
            cached_at: now,
        });
        match self.retention {
            Retention::For(retention) => {
                while self
                    .entries
                    .front()
                    .is_some_and(|entry| now.saturating_sub(entry.cached_at) > retention)
                {
                    self.entries.pop_front();
                }
            }
            Retention::NewestOnly => {
                while self.entries.len() > 1 {
                    self.entries.pop_front();
                }
            }
        }
    }

    pub(super) fn resolve(&self, start: Location, end: Location) -> FetchRange {
        let largest = self.entries.back().map(|entry| entry.object.location);
        let end_location = match resolve_fetch_range(start, end, largest) {
            Ok(end_location) => end_location,
            Err(FetchRangeError::NothingPublished) => return FetchRange::NoObjects,
            Err(FetchRangeError::EmptyRange | FetchRangeError::StartAfterLargest) => {
                return FetchRange::InvalidRange;
            }
        };
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
            end_location,
        }
    }
}

fn before_end(location: Location, end: Location) -> bool {
    location.group_id < end.group_id
        || (location.group_id == end.group_id
            && (end.object_id == 0 || location.object_id < end.object_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETENTION: Duration = Duration::from_secs(30);
    const START: Duration = Duration::from_secs(1_000);

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
        let mut cache = ObjectCache::new(Retention::For(RETENTION));
        for (group_id, object_id) in [(7, 0), (7, 1), (8, 0), (8, 1), (8, 2), (9, 0)] {
            cache.insert(object(group_id, object_id), START);
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
    fn keeps_only_the_newest_object_when_told_to_and_never_expires_it() {
        // Arrange
        let mut cache = ObjectCache::new(Retention::NewestOnly);
        cache.insert(object(1, 0), START);

        // Act
        cache.insert(object(2, 0), START + Duration::from_secs(3_600));

        // Assert
        assert_eq!(
            locations(cache.resolve(location(0, 0), location(u64::MAX, 0))),
            (vec![(2, 0)], location(2, 1))
        );
    }

    #[test]
    fn reports_no_objects_for_an_empty_cache_or_an_evicted_range() {
        // Arrange
        let mut cache = ObjectCache::new(Retention::For(RETENTION));
        let empty = cache.resolve(location(0, 0), location(1, 0));
        cache.insert(object(1, 0), START);
        cache.insert(object(2, 0), START + RETENTION + Duration::from_secs(1));

        // Act
        let evicted = cache.resolve(location(1, 0), location(1, 0));
        let (kept, _) = locations(cache.resolve(location(1, 0), location(3, 0)));

        // Assert
        assert_eq!(empty, FetchRange::NoObjects);
        assert_eq!(evicted, FetchRange::NoObjects);
        assert_eq!(kept, [(2, 0)]);
    }
}
