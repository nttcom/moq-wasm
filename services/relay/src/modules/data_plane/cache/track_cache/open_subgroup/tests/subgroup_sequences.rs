//! Checks the cache's subgroup bookkeeping against a model after every
//! operation, over random sequences of live streams opening, delivering,
//! finishing and resetting subgroups, FETCH fills and evictions. Several
//! streams may deliver one subgroup, as publishers of one track send the same
//! objects (§2.1); each delivers in ascending order and may start late or skip
//! ids.
//!
//! After each operation, the model decides for every subgroup and run what a
//! live egress reading it gets next, which positions are known and the largest
//! location; each opening checks the run it announces.

use std::{
    collections::{BTreeMap, BTreeSet},
    pin::pin,
    task::{Context, Poll, Waker},
    time::Duration,
};

use moqt::object::Location;
use proptest::{collection::vec, prelude::*};
use tokio::sync::broadcast::Receiver;

use crate::modules::{
    data_plane::cache::{
        subgroup_key::SubgroupKey,
        track_cache::{NextObject, OpenSubgroupGuard, SubgroupRun, TrackCache, location},
    },
    test_support::relay_harness::fixtures::cached_object::{
        stream_object_in_subgroup, subgroup_key as key,
    },
};

const GROUPS: u64 = 2;
const SUBGROUPS: u64 = 2;
const STREAMS: usize = 4;
/// The sequence number of a subgroup's last object.
const LAST_SEQUENCE: u64 = 5;
/// Stands in for every Object ID from `MIDDLE` to `u64::MAX - SUBGROUPS - 1`,
/// where no object is.
const MIDDLE: u64 = SUBGROUPS * LAST_SEQUENCE;
const TTL: Duration = Duration::from_secs(10);

/// Every position of a group the model decides, in order; only a group
/// closing decides `u64::MAX`.
fn positions() -> impl Iterator<Item = u64> {
    (0..MIDDLE).chain([MIDDLE, u64::MAX - 2, u64::MAX - 1, u64::MAX])
}

/// Subgroup `s` holds `SUBGROUPS * n + s` below `LAST_SEQUENCE` and
/// `u64::MAX - SUBGROUPS + s` at it, so subgroups of one group never share an
/// ID and no object is at `u64::MAX`.
fn object_id(subgroup_id: u64, sequence: u64) -> u64 {
    if sequence == LAST_SEQUENCE {
        u64::MAX - SUBGROUPS + subgroup_id
    } else {
        SUBGROUPS * sequence + subgroup_id
    }
}

/// The exclusive End Location of the positions `position` stands for; for
/// `u64::MAX` it is Object ID 0, which ends with the group.
fn end_of(group_id: u64, position: u64) -> Location {
    match position {
        MIDDLE => location(group_id, u64::MAX - SUBGROUPS),
        u64::MAX => location(group_id, 0),
        _ => location(group_id, position + 1),
    }
}

#[derive(Clone, Debug)]
enum Operation {
    Open {
        stream: usize,
        group_id: u64,
        subgroup_id: u64,
    },
    /// The stream's next object, `step` sequence numbers after its last one
    /// (or at `step - 1` for its first).
    Deliver { stream: usize, step: u64 },
    /// FIN: the stream ended cleanly.
    Finish { stream: usize },
    /// The stream ended without a FIN: reset, stop or decode error.
    Reset { stream: usize },
    /// A FETCH fill stores an object without any stream.
    Fill {
        group_id: u64,
        subgroup_id: u64,
        sequence: u64,
    },
    /// Every cached object passes the TTL and is evicted.
    Evict,
}

fn operation() -> impl Strategy<Value = Operation> {
    let stream = 0..STREAMS;
    // Mostly the next object, sometimes a skipped id or a late start.
    let step = prop_oneof![6 => Just(1u64), 2 => Just(2u64), 1 => Just(3u64)];
    prop_oneof![
        4 => (stream.clone(), 0..GROUPS, 0..SUBGROUPS).prop_map(|(stream, group_id, subgroup_id)| {
            Operation::Open { stream, group_id, subgroup_id }
        }),
        8 => (stream.clone(), step).prop_map(|(stream, step)| Operation::Deliver { stream, step }),
        3 => stream.clone().prop_map(|stream| Operation::Finish { stream }),
        2 => stream.prop_map(|stream| Operation::Reset { stream }),
        1 => (0..GROUPS, 0..SUBGROUPS, 0..=LAST_SEQUENCE).prop_map(
            |(group_id, subgroup_id, sequence)| Operation::Fill { group_id, subgroup_id, sequence }
        ),
        1 => Just(Operation::Evict),
    ]
}

/// The streams open on a subgroup since it last had none.
#[derive(Debug)]
struct ModelOpening {
    id: u64,
    streams: usize,
    /// The smallest first Object ID any stream of the opening delivered.
    earliest_first_object_id: Option<u64>,
}

#[derive(Debug, Default)]
struct ModelSubgroup {
    opening: Option<ModelOpening>,
    finished: bool,
    aborted: bool,
    /// The first object of each run after the first.
    reopened_runs: Vec<u64>,
}

impl ModelSubgroup {
    fn latest_run_first_object_id(&self) -> u64 {
        self.reopened_runs.last().copied().unwrap_or(0)
    }
}

struct Stream<'a> {
    guard: OpenSubgroupGuard<'a>,
    group_id: u64,
    subgroup_id: u64,
    /// The opening it belongs to; `None` for a stream of a finished subgroup.
    opening_id: Option<u64>,
    first_object_id: Option<u64>,
    last_sequence: Option<u64>,
}

struct Harness<'a> {
    cache: &'a TrackCache,
    runtime: &'a tokio::runtime::Runtime,
    subgroup_opened_receiver: Receiver<SubgroupRun>,
    streams: Vec<Option<Stream<'a>>>,
    subgroups: BTreeMap<(u64, u64), ModelSubgroup>,
    /// Cached objects as group, subgroup and Object ID.
    objects: BTreeSet<(u64, u64, u64)>,
    /// Known positions of `positions()`.
    known: BTreeSet<(u64, u64)>,
    /// Per live group, one past the largest Object ID a stream delivered
    /// while the group was live.
    live_frontiers: BTreeMap<u64, u64>,
    next_opening_id: u64,
}

impl<'a> Harness<'a> {
    fn new(cache: &'a TrackCache, runtime: &'a tokio::runtime::Runtime) -> Self {
        Self {
            cache,
            runtime,
            subgroup_opened_receiver: cache.subscribe_subgroup_opened(),
            streams: (0..STREAMS).map(|_| None).collect(),
            subgroups: (0..GROUPS)
                .flat_map(|group_id| (0..SUBGROUPS).map(move |subgroup_id| (group_id, subgroup_id)))
                .map(|subgroup| (subgroup, ModelSubgroup::default()))
                .collect(),
            objects: BTreeSet::new(),
            known: BTreeSet::new(),
            live_frontiers: BTreeMap::new(),
            next_opening_id: 0,
        }
    }

    fn group_is_live(&self, group_id: u64) -> bool {
        (0..SUBGROUPS).any(|subgroup_id| self.subgroups[&(group_id, subgroup_id)].opening.is_some())
    }

    fn last_cached_object_id(&self, group_id: u64, subgroup_id: u64) -> Option<u64> {
        self.objects
            .range((group_id, subgroup_id, 0)..=(group_id, subgroup_id, u64::MAX))
            .next_back()
            .map(|(_, _, object_id)| *object_id)
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), TestCaseError> {
        match *operation {
            Operation::Open {
                stream,
                group_id,
                subgroup_id,
            } => {
                if self.streams[stream].is_none() {
                    self.open(stream, group_id, subgroup_id)?;
                }
            }
            Operation::Deliver { stream, step } => self.deliver(stream, step),
            Operation::Finish { stream } => {
                if let Some(opened) = self.streams[stream].take() {
                    self.close(&opened, true);
                    opened.guard.finish();
                }
            }
            Operation::Reset { stream } => {
                if let Some(opened) = self.streams[stream].take() {
                    self.close(&opened, false);
                    drop(opened.guard);
                }
            }
            Operation::Fill {
                group_id,
                subgroup_id,
                sequence,
            } => {
                let object_id = object_id(subgroup_id, sequence);
                let _ =
                    self.cache
                        .insert(stream_object_in_subgroup(group_id, subgroup_id, object_id));
                self.objects.insert((group_id, subgroup_id, object_id));
            }
            Operation::Evict => {
                self.runtime
                    .block_on(tokio::time::advance(TTL + Duration::from_secs(1)));
                self.cache.evict(TTL);
                for (group_id, _, object_id) in std::mem::take(&mut self.objects) {
                    self.known.remove(&(group_id, object_id));
                }
                // A group with neither objects nor open subgroups is forgotten.
                for group_id in 0..GROUPS {
                    if !self.group_is_live(group_id) {
                        for subgroup_id in 0..SUBGROUPS {
                            self.subgroups
                                .insert((group_id, subgroup_id), ModelSubgroup::default());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn open(
        &mut self,
        stream: usize,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<(), TestCaseError> {
        let guard = self.cache.open_subgroup(key(group_id, subgroup_id));
        let announced = self.subgroup_opened_receiver.try_recv().ok();
        let after_cached_objects = self
            .last_cached_object_id(group_id, subgroup_id)
            .map_or(0, |object_id| object_id.saturating_add(1));
        let group_was_live = self.group_is_live(group_id);
        let subgroup = self
            .subgroups
            .get_mut(&(group_id, subgroup_id))
            .unwrap_or_else(|| unreachable!("the model holds every subgroup"));
        let opening_id = if subgroup.finished {
            // A later stream only re-delivers objects already complete.
            None
        } else {
            if subgroup.opening.is_none() && subgroup.aborted {
                // The new run starts past every object the earlier runs left.
                subgroup.aborted = false;
                let first_object_id =
                    after_cached_objects.max(subgroup.latest_run_first_object_id());
                subgroup.reopened_runs.push(first_object_id);
            }
            let opening = subgroup.opening.get_or_insert_with(|| {
                self.next_opening_id += 1;
                ModelOpening {
                    id: self.next_opening_id,
                    streams: 0,
                    earliest_first_object_id: None,
                }
            });
            opening.streams += 1;
            Some(opening.id)
        };
        let expected = opening_id.map(|_| SubgroupRun {
            key: key(group_id, subgroup_id),
            generation: subgroup.reopened_runs.len(),
            first_object_id: subgroup.latest_run_first_object_id(),
        });
        prop_assert_eq!(announced, expected, "run announced on opening");
        if !group_was_live && opening_id.is_some() {
            self.live_frontiers.insert(group_id, 0);
        }
        self.streams[stream] = Some(Stream {
            guard,
            group_id,
            subgroup_id,
            opening_id,
            first_object_id: None,
            last_sequence: None,
        });
        Ok(())
    }

    fn deliver(&mut self, stream: usize, step: u64) {
        let Some(opened) = self.streams[stream].as_mut() else {
            return;
        };
        let sequence = opened.last_sequence.map_or(step - 1, |last| last + step);
        if sequence > LAST_SEQUENCE {
            return;
        }
        let (group_id, subgroup_id) = (opened.group_id, opened.subgroup_id);
        let object_id = object_id(subgroup_id, sequence);
        let _ = opened
            .guard
            .insert(stream_object_in_subgroup(group_id, subgroup_id, object_id));
        opened.last_sequence = Some(sequence);
        if opened.first_object_id.is_none() {
            opened.first_object_id = Some(object_id);
            let subgroup = self
                .subgroups
                .get_mut(&(group_id, subgroup_id))
                .unwrap_or_else(|| unreachable!("the model holds every subgroup"));
            if let Some(opening) = subgroup
                .opening
                .as_mut()
                .filter(|opening| Some(opening.id) == opened.opening_id)
            {
                opening.earliest_first_object_id = Some(
                    opening
                        .earliest_first_object_id
                        .map_or(object_id, |earliest| earliest.min(object_id)),
                );
            }
        }
        self.objects.insert((group_id, subgroup_id, object_id));
        // A live object proves only its own position (§10.4.2).
        self.known.insert((group_id, object_id));
        if let Some(frontier) = self.live_frontiers.get_mut(&group_id) {
            *frontier = (*frontier).max(object_id.saturating_add(1));
        }
    }

    fn close(&mut self, stream: &Stream<'_>, fin: bool) {
        let group_id = stream.group_id;
        let subgroup = self
            .subgroups
            .get_mut(&(group_id, stream.subgroup_id))
            .unwrap_or_else(|| unreachable!("the model holds every subgroup"));
        // A stream of a finished subgroup or of an opening that already closed
        // closes nothing.
        let Some(opening) = subgroup
            .opening
            .as_mut()
            .filter(|opening| Some(opening.id) == stream.opening_id)
        else {
            return;
        };
        opening.streams -= 1;
        // A FIN completes the subgroup, even with other streams open, only when
        // its stream started at the opening's earliest first object or no stream
        // delivered one.
        let completes = fin
            && opening.earliest_first_object_id.is_none_or(|earliest| {
                stream
                    .first_object_id
                    .is_some_and(|first| first <= earliest)
            });
        if opening.streams > 0 && !completes {
            return;
        }
        subgroup.opening = None;
        if completes {
            subgroup.finished = true;
        } else {
            subgroup.aborted = true;
        }
        if self.group_is_live(group_id) {
            return;
        }
        let frontier = self
            .live_frontiers
            .remove(&group_id)
            .unwrap_or_else(|| unreachable!("a live group has a frontier"));
        let group_aborted =
            (0..SUBGROUPS).any(|subgroup_id| self.subgroups[&(group_id, subgroup_id)].aborted);
        if !group_aborted {
            // The last subgroup closing with none aborted decides every position
            // from the live frontier on.
            self.known.extend(
                positions()
                    .filter(|position| *position >= frontier)
                    .map(|position| (group_id, position)),
            );
        }
    }

    fn expected_next(&self, group_id: u64, subgroup_id: u64, generation: usize, from: u64) -> Next {
        let subgroup = &self.subgroups[&(group_id, subgroup_id)];
        let run_end = subgroup.reopened_runs.get(generation).copied();
        let next = self
            .objects
            .range((group_id, subgroup_id, from)..=(group_id, subgroup_id, u64::MAX))
            .map(|(_, _, object_id)| *object_id)
            .find(|object_id| run_end.is_none_or(|end| *object_id < end));
        match next {
            Some(object_id) => Next::Object(object_id),
            // A superseded run never continues into the next one (§10.4.3).
            None if run_end.is_some() => Next::Aborted,
            None if subgroup.opening.is_some() => Next::Waits,
            None if subgroup.aborted => Next::Aborted,
            None => Next::Finished,
        }
    }

    fn check(&self) -> Result<(), TestCaseError> {
        for ((group_id, subgroup_id), subgroup) in &self.subgroups {
            for generation in 0..=subgroup.reopened_runs.len() {
                for from in positions() {
                    let actual =
                        poll_next(self.cache, key(*group_id, *subgroup_id), generation, from);
                    prop_assert_eq!(
                        actual,
                        self.expected_next(*group_id, *subgroup_id, generation, from),
                        "next object of group {} subgroup {} run {} from {}",
                        group_id,
                        subgroup_id,
                        generation,
                        from
                    );
                }
            }
        }
        for group_id in 0..GROUPS {
            for position in positions() {
                let at = location(group_id, position);
                prop_assert_eq!(
                    self.cache.covers(at, end_of(group_id, position)),
                    self.known.contains(&(group_id, position)),
                    "knowledge of {:?}",
                    at
                );
            }
        }
        prop_assert_eq!(
            self.cache.largest_location(),
            self.objects
                .iter()
                .map(|(group_id, _, object_id)| location(*group_id, *object_id))
                .max()
        );
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Next {
    Object(u64),
    Finished,
    Aborted,
    Waits,
}

fn poll_next(cache: &TrackCache, key: SubgroupKey, generation: usize, from: u64) -> Next {
    let next = pin!(cache.next_subgroup_object_or_wait(key, generation, from));
    match next.poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Pending => Next::Waits,
        Poll::Ready(Ok(NextObject::Object(object))) => {
            Next::Object(object.object.location.object_id)
        }
        Poll::Ready(Ok(NextObject::Finished)) => Next::Finished,
        Poll::Ready(Ok(NextObject::Aborted)) => Next::Aborted,
        Poll::Ready(other) => {
            unreachable!("no object is arriving and the track is well formed: {other:?}")
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn the_cache_keeps_its_subgroup_bookkeeping_over_any_stream_sequence(
        operations in vec(operation(), 1..60)
    ) {
        // Arrange: eviction advances the paused clock
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let cache = TrackCache::new();
        let mut harness = Harness::new(&cache, &runtime);

        for operation in &operations {
            // Act
            harness.apply(operation)?;

            // Assert
            harness.check()?;
        }
    }
}
