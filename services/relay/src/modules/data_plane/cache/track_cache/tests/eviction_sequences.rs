//! Checks what eviction keeps against a model after every operation, over
//! random sequences of live and FETCH-filled objects, live ingest, the
//! publisher leaving and returning, time passing and eviction passes.
//!
//! The model drops an object once it reaches its MAX_CACHE_DURATION or,
//! without one, is older than the TTL, except the newest object while live
//! ingest runs; once `PUBLISHER_GONE_GRACE` passes after the publisher left
//! with no live ingest, it drops every object. A removed object takes its
//! known position and payload bytes with it.

use std::{collections::BTreeMap, time::Duration};

use moqt::{TrackProperties, object::Location};
use proptest::{collection::vec, prelude::*, sample::select};
use tokio::time::Instant;

use crate::modules::{
    data_plane::cache::{
        cached_object::CacheStamp,
        track_cache::{CacheOccupancy, PUBLISHER_GONE_GRACE, TrackCache, location},
    },
    test_support::relay_harness::fixtures::cached_object::stream_object,
};

const TTL: Duration = Duration::from_secs(10);
const GROUPS: u64 = 3;
/// `u64::MAX - 1` rather than `u64::MAX`, so `object_id + 1` is a valid End
/// Location.
const OBJECT_IDS: [u64; 4] = [0, 1, 2, u64::MAX - 1];
/// The payload length of every object `stream_object` makes.
const PAYLOAD_LEN: u64 = b"payload".len() as u64;

#[derive(Clone, Debug)]
enum Operation {
    /// An object live ingest received, or else a FETCH filled in.
    Insert {
        at: Location,
        max_cache_duration: Option<Duration>,
        live: bool,
    },
    BeginLiveIngest,
    EndLiveIngest,
    PublisherLeft,
    PublisherStarted,
    Advance(Duration),
    Evict,
}

fn at() -> impl Strategy<Value = Location> {
    (0..GROUPS, select(OBJECT_IDS.to_vec()))
        .prop_map(|(group_id, object_id)| location(group_id, object_id))
}

/// Durations around the TTL and `PUBLISHER_GONE_GRACE`, where a pass first
/// keeps and then drops an object.
fn duration() -> impl Strategy<Value = Duration> {
    select(vec![0, 1, 5, 9, 10, 11]).prop_map(Duration::from_secs)
}

fn operation() -> impl Strategy<Value = Operation> {
    let max_cache_duration = prop_oneof![3 => Just(None), 1 => duration().prop_map(Some)];
    prop_oneof![
        6 => (at(), max_cache_duration, prop::bool::weighted(2.0 / 3.0)).prop_map(
            |(at, max_cache_duration, live)| Operation::Insert { at, max_cache_duration, live }
        ),
        1 => Just(Operation::BeginLiveIngest),
        1 => Just(Operation::EndLiveIngest),
        1 => Just(Operation::PublisherLeft),
        1 => Just(Operation::PublisherStarted),
        3 => duration().prop_map(Operation::Advance),
        3 => Just(Operation::Evict),
    ]
}

#[derive(Debug)]
struct ModelObject {
    received_at: Instant,
    expires_at: Option<Instant>,
    /// Live ingest delivered it, so its position is known (§10.4.2).
    live: bool,
}

struct Harness<'a> {
    cache: TrackCache,
    runtime: &'a tokio::runtime::Runtime,
    objects: BTreeMap<Location, ModelObject>,
    live_ingests: usize,
    publisher_left_at: Option<Instant>,
    eviction_generation: u64,
}

impl<'a> Harness<'a> {
    fn new(runtime: &'a tokio::runtime::Runtime) -> Self {
        Self {
            cache: TrackCache::new(),
            runtime,
            objects: BTreeMap::new(),
            live_ingests: 0,
            publisher_left_at: None,
            eviction_generation: 0,
        }
    }

    fn insert(&mut self, at: Location, max_cache_duration: Option<Duration>, live: bool) {
        let object = CacheStamp {
            max_cache_duration,
            persist: false,
            status_may_change: false,
        }
        .apply(stream_object(at.group_id, at.object_id));
        let model = ModelObject {
            received_at: object.received_at,
            expires_at: max_cache_duration.map(|duration| object.received_at + duration),
            live,
        };
        let _ = if live {
            self.cache.insert_live(object)
        } else {
            self.cache.insert(object)
        };
        // An identical duplicate leaves the first copy.
        self.objects.entry(at).or_insert(model).live |= live;
    }

    fn apply(&mut self, operation: &Operation) {
        match *operation {
            Operation::Insert {
                at,
                max_cache_duration,
                live,
            } => self.insert(at, max_cache_duration, live),
            Operation::BeginLiveIngest => {
                self.cache.begin_live_ingest();
                self.live_ingests += 1;
            }
            Operation::EndLiveIngest => {
                if self.live_ingests > 0 {
                    self.cache.end_live_ingest();
                    self.live_ingests -= 1;
                }
            }
            Operation::PublisherLeft => {
                self.cache.publisher_left();
                self.publisher_left_at = Some(Instant::now());
            }
            Operation::PublisherStarted => {
                self.cache.publisher_started(1, TrackProperties::default());
                self.publisher_left_at = None;
            }
            Operation::Advance(duration) => self.runtime.block_on(tokio::time::advance(duration)),
            Operation::Evict => {
                self.cache.evict(TTL);
                self.evict();
            }
        }
    }

    fn evict(&mut self) {
        let now = Instant::now();
        let live = self.live_ingests > 0;
        // A late subscriber joins a live track at its newest object (§9.8).
        let newest_of_live_track = live
            .then(|| self.objects.last_key_value().map(|(at, _)| *at))
            .flatten();
        let purge = !live
            && self
                .publisher_left_at
                .is_some_and(|left_at| now.duration_since(left_at) >= PUBLISHER_GONE_GRACE);
        if purge {
            self.publisher_left_at = None;
        }
        let removed: Vec<Location> = self
            .objects
            .iter()
            .filter(|(at, object)| {
                purge
                    || match object.expires_at {
                        // MAX_CACHE_DURATION replaces the TTL (§9.2.1.3).
                        Some(expires_at) => now >= expires_at,
                        None => {
                            now.duration_since(object.received_at) > TTL
                                && Some(**at) != newest_of_live_track
                        }
                    }
            })
            .map(|(at, _)| *at)
            .collect();
        for at in &removed {
            self.objects.remove(at);
        }
        if !removed.is_empty() {
            self.eviction_generation += 1;
        }
    }

    fn check(&self) -> Result<(), TestCaseError> {
        let cached: Vec<Location> = self.cache.read().objects.keys().copied().collect();
        prop_assert_eq!(
            cached,
            self.objects.keys().copied().collect::<Vec<_>>(),
            "cached objects"
        );
        prop_assert_eq!(
            self.cache.occupancy(),
            CacheOccupancy {
                objects: self.objects.len() as u64,
                payload_bytes: self.objects.len() as u64 * PAYLOAD_LEN,
            }
        );
        prop_assert_eq!(
            self.cache.eviction_generation(),
            self.eviction_generation,
            "eviction generation"
        );
        prop_assert_eq!(self.cache.is_removable(TTL), self.objects.is_empty());
        for group_id in 0..GROUPS {
            for object_id in OBJECT_IDS {
                let at = location(group_id, object_id);
                prop_assert_eq!(
                    self.cache.covers(at, location(group_id, object_id + 1)),
                    self.objects.get(&at).is_some_and(|object| object.live),
                    "knowledge of {:?}",
                    at
                );
            }
        }
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn eviction_keeps_what_the_model_keeps_over_any_sequence(
        operations in vec(operation(), 1..60)
    ) {
        // Arrange: operations advance the paused clock
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let mut harness = Harness::new(&runtime);

        for operation in &operations {
            // Act
            harness.apply(operation);

            // Assert
            harness.check()?;
        }
    }
}
