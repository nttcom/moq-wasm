use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use crate::modules::relay::{
    cache::{cached_object::CachedObject, known_ranges::KnownRanges},
    types::SubgroupKey,
};

use super::location;

/// What live ingest has told us about a group whose subgroups are still open.
#[derive(Default)]
pub(super) struct LiveGroup {
    pub(super) open_subgroups: HashMap<SubgroupKey, usize>,
    pub(super) largest_seen_object_id: Option<u64>,
}

impl LiveGroup {
    pub(super) fn has_open_stream(&self) -> bool {
        self.open_subgroups
            .keys()
            .any(|key| matches!(key, SubgroupKey::Stream { .. }))
    }

    /// First position live ingest has not yet decided: the group head, or just
    /// past the largest object seen. Positions below it that were evicted must
    /// stay unknown, so no later claim may start before this.
    pub(super) fn knowledge_frontier(&self, group_id: u64) -> moqt::Location {
        location(
            group_id,
            self.largest_seen_object_id
                .map_or(0, |object_id| object_id.saturating_add(1)),
        )
    }
}

#[derive(Default)]
pub(super) struct Ledger {
    pub(super) objects: BTreeMap<moqt::Location, Arc<CachedObject>>,
    pub(super) live_groups: HashMap<u64, LiveGroup>,
    pub(super) known_ranges: KnownRanges,
}

impl Ledger {
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

    pub(super) fn register_live_object(&mut self, location: moqt::Location) {
        let live = self.live_groups.get_mut(&location.group_id);
        let frontier = live
            .as_ref()
            .map_or(super::location(location.group_id, 0), |live| {
                live.knowledge_frontier(location.group_id)
            });
        let start = frontier.min(location);
        self.known_ranges.insert(
            start,
            super::location(location.group_id, location.object_id.saturating_add(1)),
        );
        if let Some(live) = live {
            live.largest_seen_object_id = Some(
                live.largest_seen_object_id
                    .map_or(location.object_id, |seen| seen.max(location.object_id)),
            );
        }
    }

    pub(super) fn group_objects(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> impl Iterator<Item = &Arc<CachedObject>> {
        self.objects
            .range(location(group_id, from_object_id)..=location(group_id, u64::MAX))
            .map(|(_, object)| object)
    }

    pub(super) fn next_object(
        &self,
        key: SubgroupKey,
        from_object_id: u64,
    ) -> Option<Arc<CachedObject>> {
        self.group_objects(key.group_id(), from_object_id)
            .find(|object| object.subgroup_key() == key)
            .cloned()
    }

    pub(super) fn next_object_in_group(
        &self,
        group_id: u64,
        from_object_id: u64,
    ) -> Option<Arc<CachedObject>> {
        self.group_objects(group_id, from_object_id).next().cloned()
    }

    pub(super) fn has_open_subgroup_in_group(&self, group_id: u64) -> bool {
        self.live_groups.contains_key(&group_id)
    }

    pub(super) fn has_group(&self, group_id: u64) -> bool {
        self.next_object_in_group(group_id, 0).is_some()
            || self.has_open_subgroup_in_group(group_id)
    }

    pub(super) fn subgroups_in_group(&self, group_id: u64) -> Vec<SubgroupKey> {
        let mut keys: BTreeSet<SubgroupKey> = self
            .group_objects(group_id, 0)
            .map(|object| object.subgroup_key())
            .collect();
        keys.extend(self.open_keys_in_group(group_id));
        keys.into_iter().collect()
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
