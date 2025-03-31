use super::{CacheId, SubgroupId, SubgroupStreamId};
use moqt_core::messages::data_streams::subgroup_stream;
use std::{collections::HashMap, time::Duration};
use ttl_cache::TtlCache;

#[derive(Clone)]
pub(crate) struct SubgroupStreamsCache {
    streams: HashMap<SubgroupStreamId, SubgroupStreamCache>,
}

impl SubgroupStreamsCache {
    pub(crate) fn new() -> Self {
        let streams = HashMap::new();

        Self { streams }
    }

    pub(crate) fn set_subgroup_stream(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        header: subgroup_stream::Header,
        max_cache_size: usize,
    ) {
        let stream = SubgroupStreamCache::new(header, max_cache_size);
        let subgroup_stream_id = (group_id, subgroup_id);

        self.streams.insert(subgroup_stream_id, stream);
    }

    pub(crate) fn insert_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        object: subgroup_stream::Object,
        duration: u64,
    ) {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.insert_object(object, duration);
    }

    pub(crate) fn get_header(&self, group_id: u64, subgroup_id: u64) -> subgroup_stream::Header {
        let subgroup_stream_id = (group_id, subgroup_id);
        self.streams
            .get(&subgroup_stream_id)
            .map(|stream| stream.get_header())
            .unwrap()
    }

    pub(crate) fn get_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_object(object_id)
    }

    pub(crate) fn get_next_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        cache_id: CacheId,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_next_object(cache_id)
    }

    pub(crate) fn get_first_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_first_object()
    }

    pub(crate) fn get_latest_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_latest_object()
    }

    pub(crate) fn get_largest_group_id(&mut self) -> Option<u64> {
        self.streams.iter().map(|((gid, _), _)| *gid).max()
    }

    pub(crate) fn get_largest_object_id(&mut self) -> Option<u64> {
        let largest_group_id = self.get_largest_group_id()?;

        let subgroup_ids = self.get_all_subgroup_ids(largest_group_id);

        let mut largest_object_id = None;
        for subgroup_id in subgroup_ids.iter().rev() {
            let subgroup_stream_id = (largest_group_id, *subgroup_id);
            let object_id = self
                .streams
                .get_mut(&subgroup_stream_id)
                .unwrap()
                .get_largest_object_id();

            if largest_object_id.is_none() || object_id > largest_object_id.unwrap() {
                largest_object_id = Some(object_id);
            }
        }

        largest_object_id
    }

    pub(crate) fn get_all_subgroup_ids(&mut self, group_id: u64) -> Vec<SubgroupId> {
        let mut subgroup_ids: Vec<SubgroupId> = self
            .streams
            .iter()
            .filter_map(
                |((gid, sgid), _)| {
                    if *gid == group_id {
                        Some(*sgid)
                    } else {
                        None
                    }
                },
            )
            .collect();

        subgroup_ids.sort_unstable();
        subgroup_ids
    }
}

#[derive(Clone)]
struct SubgroupStreamCache {
    header: subgroup_stream::Header,
    objects: TtlCache<CacheId, subgroup_stream::Object>,
    next_cache_id: CacheId,
}

impl SubgroupStreamCache {
    fn new(header: subgroup_stream::Header, max_cache_size: usize) -> Self {
        let objects = TtlCache::new(max_cache_size);

        Self {
            header,
            objects,
            next_cache_id: 0,
        }
    }

    fn insert_object(&mut self, object: subgroup_stream::Object, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(self.next_cache_id, object, ttl);
        self.next_cache_id += 1;
    }

    fn get_header(&self) -> subgroup_stream::Header {
        self.header.clone()
    }

    fn get_object(&mut self, object_id: u64) -> Option<(CacheId, subgroup_stream::Object)> {
        self.objects.iter().find_map(|(k, v)| {
            if v.object_id() == object_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_next_object(&mut self, cache_id: CacheId) -> Option<(CacheId, subgroup_stream::Object)> {
        let next_cache_id = cache_id + 1;
        self.objects.iter().find_map(|(k, v)| {
            if *k == next_cache_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_first_object(&mut self) -> Option<(CacheId, subgroup_stream::Object)> {
        self.objects.iter().next().map(|(k, v)| (*k, v.clone()))
    }

    fn get_latest_object(&mut self) -> Option<(CacheId, subgroup_stream::Object)> {
        self.objects.iter().last().map(|(k, v)| (*k, v.clone()))
    }

    fn get_largest_object_id(&mut self) -> u64 {
        self.objects
            .iter()
            .map(|(_, v)| v.object_id())
            .max()
            .unwrap()
    }
}
