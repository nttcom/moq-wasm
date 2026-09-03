use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use crate::modules::relay::{
    cache::{cached_object::CachedObject, known_ranges::KnownRanges},
    types::SubgroupKey,
};

use super::location;

#[derive(Default)]
pub(super) struct Ledger {
    pub(super) objects: BTreeMap<moqt::Location, Arc<CachedObject>>,
    pub(super) open_subgroups: HashMap<SubgroupKey, usize>,
    pub(super) known_ranges: KnownRanges,
}

impl Ledger {
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
        self.open_subgroups
            .keys()
            .any(|key| key.group_id() == group_id)
    }

    pub(super) fn has_open_stream_in_group(&self, group_id: u64) -> bool {
        self.open_subgroups
            .keys()
            .any(|key| matches!(key, SubgroupKey::Stream { group_id: g, .. } if *g == group_id))
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
        keys.extend(
            self.open_subgroups
                .keys()
                .filter(|key| key.group_id() == group_id),
        );
        keys.into_iter().collect()
    }

    pub(super) fn groups_in_range(&self, first_group_id: u64, last_group_id: u64) -> Vec<u64> {
        let mut groups: BTreeSet<u64> = self
            .objects
            .range(location(first_group_id, 0)..=location(last_group_id, u64::MAX))
            .map(|(location, _)| location.group_id)
            .collect();
        groups.extend(
            self.open_subgroups
                .keys()
                .map(|key| key.group_id())
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
