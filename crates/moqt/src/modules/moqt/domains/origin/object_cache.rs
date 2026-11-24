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

/// How long a track's cache keeps each object for FETCH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// Each object is served until this long after it was written, so
    /// `Duration::ZERO` serves none.
    For(Duration),
    /// Only the newest object is served, however old it is.
    NewestOnly,
}

pub(super) struct ObjectCache {
    retention: Retention,
    /// In the order written, so the oldest expires first.
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

    /// `now` must not be before the `now` of an earlier insert.
    pub(super) fn insert(&mut self, object: CachedObject, now: Duration) {
        if self.retention == Retention::NewestOnly {
            self.entries.clear();
        }
        self.entries.push_back(Entry {
            object,
            cached_at: now,
        });
        self.evict_expired(now);
    }

    pub(super) fn evict_expired(&mut self, now: Duration) {
        let Retention::For(retention) = self.retention else {
            return;
        };
        while self
            .entries
            .front()
            .is_some_and(|entry| now.saturating_sub(entry.cached_at) >= retention)
        {
            self.entries.pop_front();
        }
    }

    /// When the oldest object expires; `None` while no object will.
    pub(super) fn next_expiry(&self) -> Option<Duration> {
        let Retention::For(retention) = self.retention else {
            return None;
        };
        self.entries.front()?.cached_at.checked_add(retention)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serves only the objects that have not expired at `now`.
    pub(super) fn resolve(&mut self, start: Location, end: Location, now: Duration) -> FetchRange {
        self.evict_expired(now);
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
    use crate::modules::test_support::location;

    const RETENTION: Duration = Duration::from_secs(30);
    const START: Duration = Duration::from_secs(1_000);

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
        let mut cache = cache_with_two_closed_groups_and_an_open_one();

        // Act
        let (objects, end_location) =
            locations(cache.resolve(location(8, 0), location(8, 0), START));

        // Assert
        assert_eq!(objects, [(8, 0), (8, 1), (8, 2)]);
        assert_eq!(end_location, location(8, 0));
    }

    #[test]
    fn serves_an_explicit_range_across_groups_with_an_exclusive_end() {
        // Arrange
        let mut cache = cache_with_two_closed_groups_and_an_open_one();

        // Act
        let (objects, end_location) =
            locations(cache.resolve(location(7, 1), location(8, 2), START));

        // Assert
        assert_eq!(objects, [(7, 1), (8, 0), (8, 1)]);
        assert_eq!(end_location, location(8, 2));
    }

    #[test]
    fn ends_after_the_largest_object_when_the_request_reaches_the_open_group() {
        // Arrange
        let mut cache = cache_with_two_closed_groups_and_an_open_one();

        // Act
        let (whole_open_group, open_end) =
            locations(cache.resolve(location(9, 0), location(9, 0), START));
        let (beyond, beyond_end) = locations(cache.resolve(location(8, 2), location(20, 5), START));

        // Assert
        assert_eq!(whole_open_group, [(9, 0)]);
        assert_eq!(open_end, location(9, 1));
        assert_eq!(beyond, [(8, 2), (9, 0)]);
        assert_eq!(beyond_end, location(9, 1));
    }

    #[test]
    fn rejects_ranges_that_end_before_they_start_or_start_past_the_largest_object() {
        // Arrange
        let mut cache = cache_with_two_closed_groups_and_an_open_one();

        // Act / Assert
        assert_eq!(
            cache.resolve(location(8, 2), location(8, 1), START),
            FetchRange::InvalidRange
        );
        assert_eq!(
            cache.resolve(location(9, 0), location(8, 0), START),
            FetchRange::InvalidRange
        );
        assert_eq!(
            cache.resolve(location(9, 1), location(10, 0), START),
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
        let much_later = START + Duration::from_secs(7_200);

        // Assert
        assert_eq!(
            locations(cache.resolve(location(0, 0), location(u64::MAX, 0), much_later)),
            (vec![(2, 0)], location(2, 1))
        );
        assert_eq!(cache.next_expiry(), None);
    }

    #[test]
    fn reports_no_objects_for_an_empty_cache_or_an_evicted_range() {
        // Arrange
        let mut cache = ObjectCache::new(Retention::For(RETENTION));
        let empty = cache.resolve(location(0, 0), location(1, 0), START);
        cache.insert(object(1, 0), START);
        let later = START + RETENTION + Duration::from_secs(1);
        cache.insert(object(2, 0), later);

        // Act
        let evicted = cache.resolve(location(1, 0), location(1, 0), later);
        let (kept, _) = locations(cache.resolve(location(1, 0), location(3, 0), later));

        // Assert
        assert_eq!(empty, FetchRange::NoObjects);
        assert_eq!(evicted, FetchRange::NoObjects);
        assert_eq!(kept, [(2, 0)]);
    }

    #[test]
    fn serves_no_object_past_its_retention_without_a_later_insert() {
        // Arrange
        let mut cache = ObjectCache::new(Retention::For(RETENTION));
        cache.insert(object(1, 0), START);
        cache.insert(object(2, 0), START + Duration::from_secs(10));

        // Act
        let first_expired = cache.resolve(location(1, 0), location(3, 0), START + RETENTION);
        let next_expiry = cache.next_expiry();
        let all_expired = cache.resolve(
            location(1, 0),
            location(3, 0),
            START + Duration::from_secs(40),
        );

        // Assert
        assert_eq!(locations(first_expired).0, [(2, 0)]);
        assert_eq!(next_expiry, Some(START + Duration::from_secs(40)));
        assert_eq!(all_expired, FetchRange::NoObjects);
        assert!(cache.is_empty());
        assert_eq!(cache.next_expiry(), None);
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;

        #[derive(Debug, Clone, Copy)]
        enum Step {
            Insert,
            Read,
        }

        fn retention() -> impl Strategy<Value = Duration> {
            prop_oneof![
                Just(Duration::ZERO),
                Just(Duration::from_nanos(1)),
                (1..1_000u64).prop_map(Duration::from_nanos),
                Just(Duration::MAX),
                any::<u64>().prop_map(Duration::from_nanos),
            ]
        }

        /// Each step advances the clock by a gap that favours the retention's
        /// edges, where an object is about to expire.
        fn steps(retention: Duration) -> impl Strategy<Value = Vec<(Step, Duration)>> {
            let retention = u64::try_from(retention.as_nanos()).unwrap_or(u64::MAX);
            let gap = prop_oneof![
                Just(0),
                Just(1),
                Just(retention.saturating_sub(1)),
                Just(retention),
                Just(retention.saturating_add(1)),
                0..retention.saturating_mul(2).max(1),
            ]
            .prop_map(Duration::from_nanos);
            let step = prop_oneof![Just(Step::Insert), Just(Step::Read)];
            proptest::collection::vec((step, gap), 0..24)
        }

        fn retention_and_steps() -> impl Strategy<Value = (Duration, Vec<(Step, Duration)>)> {
            retention().prop_flat_map(|retention| (Just(retention), steps(retention)))
        }

        proptest! {
            #[test]
            fn serves_exactly_the_objects_younger_than_the_retention(
                (retention, steps) in retention_and_steps(),
            ) {
                // Arrange
                let mut cache = ObjectCache::new(Retention::For(retention));
                let mut written: Vec<(u64, Duration)> = Vec::new();
                let mut now = START;

                for (step, gap) in steps {
                    // Act
                    now = now.saturating_add(gap);
                    match step {
                        Step::Insert => {
                            let group_id = written.len() as u64;
                            cache.insert(object(group_id, 0), now);
                            written.push((group_id, now));
                        }
                        Step::Read => {
                            let served = match cache.resolve(location(0, 0), location(u64::MAX, 0), now) {
                                FetchRange::Serve { objects, .. } => objects
                                    .iter()
                                    .map(|object| object.location.group_id)
                                    .collect(),
                                FetchRange::NoObjects => Vec::new(),
                                FetchRange::InvalidRange => panic!("the range covers every object"),
                            };

                            // Assert: the model keeps an object while its age is below the retention
                            let alive: Vec<u64> = written
                                .iter()
                                .filter(|(_, cached_at)| (now - *cached_at) < retention)
                                .map(|(group_id, _)| *group_id)
                                .collect();
                            prop_assert_eq!(&served, &alive);
                            let oldest_alive = written
                                .iter()
                                .map(|(_, cached_at)| *cached_at)
                                .find(|cached_at| (now - *cached_at) < retention);
                            prop_assert_eq!(
                                cache.next_expiry(),
                                oldest_alive.and_then(|cached_at| cached_at.checked_add(retention))
                            );
                        }
                    }
                }
            }

            #[test]
            fn evicting_at_the_next_expiry_always_removes_an_object(
                (retention, steps) in retention_and_steps(),
            ) {
                // Arrange
                let mut cache = ObjectCache::new(Retention::For(retention));
                let mut now = START;
                for (_, gap) in steps {
                    now = now.saturating_add(gap);
                    cache.insert(object(0, 0), now);
                }
                let before = cache.entries.len();

                // Act
                let Some(next_expiry) = cache.next_expiry() else {
                    return Ok(());
                };
                cache.evict_expired(next_expiry);

                // Assert: a task that wakes at the next expiry always evicts an object
                prop_assert!(cache.entries.len() < before);
            }
        }
    }
}
