use crate::modules::data_plane::cache::{
    cached_object::CachedObject,
    subgroup_key::SubgroupKey,
    track_cache::{OpenSubgroupGuard, TrackCache, TrackMalformed},
};

/// The group a track's datagrams are filling: it stays open in the cache
/// until a datagram of another group arrives or the track's ingest ends.
pub(crate) struct DatagramGroup<'a> {
    cache: &'a TrackCache,
    open: Option<(u64, OpenSubgroupGuard<'a>)>,
}

impl<'a> DatagramGroup<'a> {
    pub(crate) fn new(cache: &'a TrackCache) -> Self {
        Self { cache, open: None }
    }

    pub(crate) fn insert(&mut self, object: CachedObject) -> Result<(), TrackMalformed> {
        let group_id = object.object.location.group_id;
        let open = match &mut self.open {
            Some((open_group_id, open)) if *open_group_id == group_id => open,
            slot => {
                if let Some((_, previous)) = slot.take() {
                    previous.finish();
                }
                let open = self.cache.open_subgroup(SubgroupKey::Datagram { group_id });
                &mut slot.insert((group_id, open)).1
            }
        };
        open.insert(object)
    }
}
