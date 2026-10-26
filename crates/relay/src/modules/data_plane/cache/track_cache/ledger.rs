use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::RangeInclusive,
    sync::Arc,
};

use tokio::time::Instant;

use crate::modules::data_plane::{
    cache::subgroup_key::SubgroupKey,
    cache::{cached_object::CachedObject, known_ranges::KnownRanges},
};

use super::{
    SubgroupRun, after, eviction_queues::EvictionQueues, location, partial_object::PartialObject,
};

/// `epoch` tells a guard opened before the subgroup last closed apart from one
/// of the current opening.
pub(super) struct OpenSubgroup {
    pub(super) guards: usize,
    pub(super) epoch: u64,
    pub(super) earliest_first_object_id: Option<u64>,
    pub(super) any_finished: bool,
}

#[derive(Default)]
pub(super) struct LiveGroup {
    pub(super) open_subgroups: HashMap<SubgroupKey, OpenSubgroup>,
    pub(super) knowledge_frontier: u64,
}

impl LiveGroup {
    pub(super) fn has_open_stream(&self) -> bool {
        self.open_subgroups
            .keys()
            .any(|key| matches!(key, SubgroupKey::Stream { .. }))
    }
}

fn subgroup_keys_of(group_id: u64) -> [RangeInclusive<SubgroupKey>; 2] {
    [
        SubgroupKey::Stream {
            group_id,
            subgroup_id: 0,
        }..=SubgroupKey::Stream {
            group_id,
            subgroup_id: u64::MAX,
        },
        SubgroupKey::Datagram { group_id }..=SubgroupKey::Datagram { group_id },
    ]
}

#[derive(Default)]
pub(super) struct Ledger {
    pub(super) objects: BTreeMap<moqt::object::Location, Arc<CachedObject>>,
    /// Objects whose payload is still arriving. A location `objects` already
    /// held when the object started never gets an entry.
    pub(super) partial_objects: BTreeMap<moqt::object::Location, Arc<PartialObject>>,
    pub(super) live_groups: HashMap<u64, LiveGroup>,
    pub(super) aborted_subgroups: BTreeSet<SubgroupKey>,
    pub(super) finished_subgroups: BTreeSet<SubgroupKey>,
    reopened_run_first_object_ids: BTreeMap<SubgroupKey, Vec<u64>>,
    /// Groups whose last open subgroup closed since the last eviction.
    pub(super) groups_closed_since_eviction: Vec<u64>,
    pub(super) known_ranges: KnownRanges,
    pub(super) eviction_queues: EvictionQueues,
    pub(super) next_open_epoch: u64,
    /// When the track's last publisher left; cleared when an ingest begins or
    /// eviction purges the track.
    pub(super) publisher_left_at: Option<Instant>,
}

impl Ledger {
    pub(super) fn is_group_aborted(&self, group_id: u64) -> bool {
        self.aborted_subgroups
            .iter()
            .any(|key| key.group_id() == group_id)
    }

    /// Checks only the groups of `removed` and the ones closed since the last
    /// call: a group vanishes only when its last object leaves or its last
    /// open subgroup closes.
    pub(super) fn forget_runs_of_vanished_groups(&mut self, removed: &[moqt::object::Location]) {
        let mut group_ids: Vec<u64> = removed.iter().map(|at| at.group_id).collect();
        group_ids.append(&mut self.groups_closed_since_eviction);
        group_ids.sort_unstable();
        group_ids.dedup();
        for group_id in group_ids {
            if self.group_objects(group_id, 0).next().is_some()
                || self.live_groups.contains_key(&group_id)
            {
                continue;
            }
            for keys in subgroup_keys_of(group_id) {
                self.aborted_subgroups
                    .extract_if(keys.clone(), |_| true)
                    .for_each(drop);
                self.finished_subgroups
                    .extract_if(keys.clone(), |_| true)
                    .for_each(drop);
                self.reopened_run_first_object_ids
                    .extract_if(keys, |_, _| true)
                    .for_each(drop);
            }
        }
    }

    /// The run a reopened stream starts begins past every cached object of the
    /// subgroup, so the object ids below it belong to the superseded runs.
    pub(super) fn start_run_if_reopening_aborted(&mut self, key: SubgroupKey) {
        if self.is_open(key) || !self.aborted_subgroups.remove(&key) {
            return;
        }
        let after_cached_objects = self
            .group_objects(key.group_id(), 0)
            .rev()
            .find(|object| object.subgroup_key() == key)
            .map_or(0, |object| {
                object.object.location.object_id.saturating_add(1)
            });
        let first_object_id = after_cached_objects.max(self.latest_run(key).first_object_id);
        self.reopened_run_first_object_ids
            .entry(key)
            .or_default()
            .push(first_object_id);
    }

    pub(super) fn latest_run(&self, key: SubgroupKey) -> SubgroupRun {
        let first_object_ids = self
            .reopened_run_first_object_ids
            .get(&key)
            .map(Vec::as_slice)
            .unwrap_or_default();
        SubgroupRun {
            key,
            generation: first_object_ids.len(),
            first_object_id: first_object_ids.last().copied().unwrap_or(0),
        }
    }

    pub(super) fn reopen_count_in_group(&self, group_id: u64) -> usize {
        self.reopened_run_first_object_ids
            .iter()
            .filter(|(key, _)| key.group_id() == group_id)
            .map(|(_, first_object_ids)| first_object_ids.len())
            .sum()
    }

    pub(super) fn superseded_run_end(&self, key: SubgroupKey, generation: usize) -> Option<u64> {
        self.reopened_run_first_object_ids
            .get(&key)?
            .get(generation)
            .copied()
    }

    pub(super) fn is_open(&self, key: SubgroupKey) -> bool {
        self.live_groups
            .get(&key.group_id())
            .is_some_and(|live| live.open_subgroups.contains_key(&key))
    }

    fn open_keys_in_group(&self, group_id: u64) -> impl Iterator<Item = SubgroupKey> {
        self.live_groups
            .get(&group_id)
            .into_iter()
            .flat_map(|live| live.open_subgroups.keys().copied())
    }

    /// A live object proves only its own position (draft-14 §10.4.2: the ids
    /// skipped by a non-zero delta cannot be inferred); the rest of the group is
    /// decided when its last subgroup closes.
    pub(super) fn register_live_object(&mut self, location: moqt::object::Location) {
        self.known_ranges.insert(location, after(location));
        if let Some(live) = self.live_groups.get_mut(&location.group_id) {
            live.knowledge_frontier = live
                .knowledge_frontier
                .max(location.object_id.saturating_add(1));
        }
    }

    pub(super) fn group_objects(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> impl DoubleEndedIterator<Item = &Arc<CachedObject>> {
        self.objects
            .range(location(group_id, from_object_id)..=location(group_id, u64::MAX))
            .map(|(_, object)| object)
    }

    /// Skips objects past their MAX_CACHE_DURATION at `now`.
    pub(super) fn next_subgroup_object(
        &self,
        key: SubgroupKey,
        from_object_id: u64,
        now: Instant,
    ) -> Option<Arc<CachedObject>> {
        self.group_objects(key.group_id(), from_object_id)
            .find(|object| object.subgroup_key() == key && !object.is_expired(now))
            .cloned()
    }

    pub(super) fn next_subgroup_partial(
        &self,
        key: SubgroupKey,
        from_object_id: u64,
    ) -> Option<Arc<PartialObject>> {
        let group_id = key.group_id();
        self.partial_objects
            .range(location(group_id, from_object_id)..=location(group_id, u64::MAX))
            .map(|(_, partial)| partial)
            .find(|partial| SubgroupKey::of(&partial.object) == key)
            .cloned()
    }

    pub(super) fn next_group_object(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> Option<Arc<CachedObject>> {
        self.group_objects(group_id, from_object_id).next().cloned()
    }

    pub(super) fn has_open_subgroup_in_group(&self, group_id: u64) -> bool {
        self.live_groups.contains_key(&group_id)
    }

    pub(super) fn subgroup_runs_in_group(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> Vec<SubgroupRun> {
        let mut keys: BTreeSet<SubgroupKey> = self
            .group_objects(group_id, from_object_id)
            .map(|object| object.subgroup_key())
            .collect();
        keys.extend(self.open_keys_in_group(group_id));
        keys.into_iter().map(|key| self.latest_run(key)).collect()
    }

    pub(super) fn groups_in_range(&self, first_group_id: u64, last_group_id: u64) -> Vec<u64> {
        let mut groups: BTreeSet<u64> = self
            .objects
            .range(location(first_group_id, 0)..=location(last_group_id, u64::MAX))
            .map(|(location, _)| location.group_id)
            .collect();
        groups.extend(
            self.live_groups
                .keys()
                .filter(|group_id| (first_group_id..=last_group_id).contains(group_id)),
        );
        groups.into_iter().collect()
    }

    pub(super) fn largest_location(&self) -> Option<moqt::object::Location> {
        self.objects.last_key_value().map(|(location, _)| *location)
    }

    pub(super) fn has_object_in(
        &self,
        start: moqt::object::Location,
        end: moqt::object::Location,
    ) -> bool {
        self.objects_in(start, end).next().is_some()
    }

    pub(super) fn has_expired_object_in(
        &self,
        start: moqt::object::Location,
        end: moqt::object::Location,
        now: Instant,
    ) -> bool {
        let range = start..KnownRanges::exclusive_end(end);
        self.eviction_queues
            .expired_candidates(now)
            .filter(|at| range.contains(at))
            .any(|at| {
                self.objects
                    .get(&at)
                    .is_some_and(|object| object.is_expired(now))
            })
    }

    fn objects_in(
        &self,
        start: moqt::object::Location,
        end: moqt::object::Location,
    ) -> impl Iterator<Item = &Arc<CachedObject>> {
        let end = KnownRanges::exclusive_end(end);
        (start < end)
            .then(|| self.objects.range(start..end).map(|(_, object)| object))
            .into_iter()
            .flatten()
    }
}
