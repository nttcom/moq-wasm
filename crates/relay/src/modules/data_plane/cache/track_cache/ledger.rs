use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use crate::modules::data_plane::{
    cache::subgroup_key::SubgroupKey,
    cache::{cached_object::CachedObject, known_ranges::KnownRanges},
};

use super::{SubgroupRun, after, location};

#[derive(Default)]
pub(super) struct LiveGroup {
    pub(super) open_subgroups: HashMap<SubgroupKey, usize>,
    pub(super) knowledge_frontier: u64,
}

impl LiveGroup {
    pub(super) fn has_open_stream(&self) -> bool {
        self.open_subgroups
            .keys()
            .any(|key| matches!(key, SubgroupKey::Stream { .. }))
    }
}

#[derive(Default)]
pub(super) struct Ledger {
    pub(super) objects: BTreeMap<moqt::Location, Arc<CachedObject>>,
    pub(super) live_groups: HashMap<u64, LiveGroup>,
    pub(super) aborted_subgroups: HashSet<SubgroupKey>,
    reopened_run_first_object_ids: HashMap<SubgroupKey, Vec<u64>>,
    pub(super) known_ranges: KnownRanges,
}

impl Ledger {
    pub(super) fn is_group_aborted(&self, group_id: u64) -> bool {
        self.aborted_subgroups
            .iter()
            .any(|key| key.group_id() == group_id)
    }

    pub(super) fn forget_runs_of_vanished_groups(&mut self) {
        let (objects, live_groups) = (&self.objects, &self.live_groups);
        let is_present = |key: &SubgroupKey| {
            let group_id = key.group_id();
            objects
                .range(location(group_id, 0)..=location(group_id, u64::MAX))
                .next()
                .is_some()
                || live_groups.contains_key(&group_id)
        };
        self.aborted_subgroups.retain(is_present);
        self.reopened_run_first_object_ids
            .retain(|key, _| is_present(key));
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
            .map_or(0, |object| object.location.object_id.saturating_add(1));
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
    pub(super) fn register_live_object(&mut self, location: moqt::Location) {
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

    pub(super) fn next_subgroup_object(
        &self,
        key: SubgroupKey,
        from_object_id: u64,
    ) -> Option<Arc<CachedObject>> {
        self.group_objects(key.group_id(), from_object_id)
            .find(|object| object.subgroup_key() == key)
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

    pub(super) fn subgroup_runs_in_group(&self, group_id: u64) -> Vec<SubgroupRun> {
        let mut keys: BTreeSet<SubgroupKey> = self
            .group_objects(group_id, 0)
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

    pub(super) fn largest_location(&self) -> Option<moqt::Location> {
        self.objects.last_key_value().map(|(location, _)| *location)
    }

    pub(super) fn has_object_in(&self, start: moqt::Location, end: moqt::Location) -> bool {
        let end = KnownRanges::exclusive_end(end);
        start < end && self.objects.range(start..end).next().is_some()
    }
}
