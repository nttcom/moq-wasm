use super::CacheId;
use moqt_core::messages::data_streams::track_stream;
use std::time::Duration;
use ttl_cache::TtlCache;

#[derive(Clone)]
pub(crate) struct TrackStreamCache {
    header: track_stream::Header,
    objects: TtlCache<CacheId, track_stream::Object>,
    next_cache_id: CacheId,
}

impl TrackStreamCache {
    pub(crate) fn new(header: track_stream::Header, max_store_size: usize) -> Self {
        let objects = TtlCache::new(max_store_size);

        Self {
            header,
            objects,
            next_cache_id: 0,
        }
    }

    pub(crate) fn insert_object(&mut self, object: track_stream::Object, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(self.next_cache_id, object, ttl);
        self.next_cache_id += 1;
    }

    pub(crate) fn get_header(&self) -> track_stream::Header {
        self.header.clone()
    }

    pub(crate) fn get_absolute_object_with_cache_id(
        &mut self,
        group_id: u64,
        object_id: u64,
    ) -> Option<(CacheId, track_stream::Object)> {
        self.objects.iter().find_map(|(k, v)| {
            if v.group_id() == group_id && v.object_id() == object_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    pub(crate) fn get_next_object_with_cache_id(
        &mut self,
        cache_id: CacheId,
    ) -> Option<(CacheId, track_stream::Object)> {
        let next_cache_id = cache_id + 1;
        self.objects.iter().find_map(|(k, v)| {
            if *k == next_cache_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    pub(crate) fn get_latest_group_with_cache_id(
        &mut self,
    ) -> Option<(CacheId, track_stream::Object)> {
        let latest_group_id = self
            .objects
            .iter()
            .last()
            .map(|(_, v)| v.group_id())
            .unwrap();

        let latest_group = self.objects.iter().filter_map(|(k, v)| {
            if v.group_id() == latest_group_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        });

        latest_group.min_by_key(|(k, v)| (v.object_id(), *k))
    }

    pub(crate) fn get_latest_object_with_cache_id(
        &mut self,
    ) -> Option<(CacheId, track_stream::Object)> {
        self.objects.iter().last().map(|(k, v)| (*k, v.clone()))
    }

    pub(crate) fn get_largest_group_id(&mut self) -> u64 {
        self.objects
            .iter()
            .map(|(_, v)| v.group_id())
            .max()
            .unwrap()
    }

    pub(crate) fn get_largest_object_id(&mut self) -> u64 {
        let largest_group_id = self.get_largest_group_id();

        self.objects
            .iter()
            .filter_map(|(_, v)| {
                if v.group_id() == largest_group_id {
                    Some(v.object_id())
                } else {
                    None
                }
            })
            .max()
            .unwrap()
    }
}
