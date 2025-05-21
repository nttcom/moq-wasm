use super::{SubgroupId, SubgroupStreamId};
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
        let cache = SubgroupStreamCache::new(header, max_cache_size);
        let subgroup_stream_id = (group_id, subgroup_id);

        self.streams.insert(subgroup_stream_id, cache);
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

    pub(crate) fn get_absolute_or_next_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
    ) -> Option<subgroup_stream::Object> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_absolute_or_next_object(object_id)
    }

    pub(crate) fn get_next_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        current_object_id: u64,
    ) -> Option<subgroup_stream::Object> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_next_object(current_object_id)
    }

    pub(crate) fn get_first_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
    ) -> Option<subgroup_stream::Object> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_first_object()
    }

    pub(crate) fn get_latest_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
    ) -> Option<subgroup_stream::Object> {
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

        let mut largest_object_id: Option<u64> = None;
        for subgroup_id in subgroup_ids.iter().rev() {
            let subgroup_stream_id = (largest_group_id, *subgroup_id);
            if let Some(object_id) = self
                .streams
                .get_mut(&subgroup_stream_id)
                .unwrap()
                .get_largest_object_id()
            {
                if largest_object_id.is_none() || object_id > largest_object_id.unwrap() {
                    largest_object_id = Some(object_id);
                }
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
                    if *gid == group_id { Some(*sgid) } else { None }
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
    objects: TtlCache<u64, subgroup_stream::Object>,
}

impl SubgroupStreamCache {
    fn new(header: subgroup_stream::Header, max_cache_size: usize) -> Self {
        let objects = TtlCache::new(max_cache_size);

        Self { header, objects }
    }

    fn insert_object(&mut self, object: subgroup_stream::Object, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(object.object_id(), object, ttl);
    }

    fn get_header(&self) -> subgroup_stream::Header {
        self.header.clone()
    }

    fn get_absolute_or_next_object(
        &mut self,
        object_id: u64,
    ) -> Option<subgroup_stream::Object> {
        if let Some(obj) = self.objects.get(&object_id) {
            return Some(obj.clone());
        }
        self.objects
            .iter()
            .filter_map(|(o_id, obj)| {
                if *o_id > object_id {
                    Some((*o_id, obj.clone()))
                } else {
                    None
                }
            })
            .min_by_key(|(o_id, _)| *o_id)
            .map(|(_, obj)| obj)
    }

    fn get_next_object(&mut self, current_object_id: u64) -> Option<subgroup_stream::Object> {
        self.objects
            .iter()
            .filter_map(|(o_id, obj)| {
                if *o_id > current_object_id {
                    Some((*o_id, obj.clone()))
                } else {
                    None
                }
            })
            .min_by_key(|(o_id, _)| *o_id)
            .map(|(_, obj)| obj)
    }

    fn get_first_object(&mut self) -> Option<subgroup_stream::Object> {
        self.objects
            .iter()
            .min_by_key(|(o_id, _)| *o_id)
            .map(|(_, obj)| obj.clone())
    }

    fn get_latest_object(&mut self) -> Option<subgroup_stream::Object> {
        self.objects
            .iter()
            .max_by_key(|(o_id, _)| *o_id)
            .map(|(_, obj)| obj.clone())
    }

    fn get_largest_object_id(&mut self) -> Option<u64> {
        self.objects.keys().max().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moqt_core::messages::data_streams::subgroup_stream::{Header, Object};

    fn create_test_header(group_id: u64, subgroup_id: u64) -> Header {
        Header::new(0, group_id, subgroup_id, 0).unwrap()
    }

    fn create_test_subgroup_object(object_id: u64, payload_byte: u8) -> Object {
        Object::new(object_id, vec![], None, vec![payload_byte]).unwrap()
    }

    #[test]
    fn test_subgroup_cache_insert_and_get_header() {
        let header = create_test_header(1, 1);
        let cache = SubgroupStreamCache::new(header.clone(), 10);
        assert_eq!(cache.get_header(), header);
    }

    #[test]
    fn test_subgroup_cache_insert_and_get_simple() {
        let header = create_test_header(1, 1);
        let mut cache = SubgroupStreamCache::new(header.clone(), 10);
        
        let obj1 = create_test_subgroup_object(1, 101);
        cache.insert_object(obj1.clone(), 1000);

        // TtlCache get is not directly exposed, we test via other methods
        assert_eq!(cache.get_absolute_or_next_object(1), Some(obj1.clone()));
        assert_eq!(cache.get_latest_object(), Some(obj1.clone()));
    }

    #[test]
    fn test_subgroup_cache_get_absolute_or_next_object() {
        let header = create_test_header(1, 1);
        let mut cache = SubgroupStreamCache::new(header.clone(), 10);

        let obj1 = create_test_subgroup_object(1, 101);
        let obj3 = create_test_subgroup_object(3, 103);
        let obj5 = create_test_subgroup_object(5, 105);

        cache.insert_object(obj1.clone(), 1000);
        cache.insert_object(obj3.clone(), 1000);
        cache.insert_object(obj5.clone(), 1000);

        // Absolute match
        assert_eq!(cache.get_absolute_or_next_object(1), Some(obj1.clone()));
        assert_eq!(cache.get_absolute_or_next_object(3), Some(obj3.clone()));
        assert_eq!(cache.get_absolute_or_next_object(5), Some(obj5.clone()));

        // Next object when absolute match fails
        assert_eq!(cache.get_absolute_or_next_object(0), Some(obj1.clone())); // Next is obj1
        assert_eq!(cache.get_absolute_or_next_object(2), Some(obj3.clone())); // Next is obj3
        assert_eq!(cache.get_absolute_or_next_object(4), Some(obj5.clone())); // Next is obj5
        
        // No object if requested object_id is greater than all existing
        assert_eq!(cache.get_absolute_or_next_object(6), None);
        
        // Empty cache
        let mut empty_cache = SubgroupStreamCache::new(header.clone(), 10);
        assert_eq!(empty_cache.get_absolute_or_next_object(1), None);
    }
    
    #[test]
    fn test_subgroup_cache_get_next_object() {
        let header = create_test_header(1, 1);
        let mut cache = SubgroupStreamCache::new(header.clone(), 10);

        let obj1 = create_test_subgroup_object(1, 101);
        let obj3 = create_test_subgroup_object(3, 103);
        let obj5 = create_test_subgroup_object(5, 105);

        cache.insert_object(obj1.clone(), 1000);
        cache.insert_object(obj3.clone(), 1000);
        cache.insert_object(obj5.clone(), 1000);

        assert_eq!(cache.get_next_object(0), Some(obj1.clone()));
        assert_eq!(cache.get_next_object(1), Some(obj3.clone()));
        assert_eq!(cache.get_next_object(2), Some(obj3.clone())); // Next after 2 is 3
        assert_eq!(cache.get_next_object(3), Some(obj5.clone()));
        assert_eq!(cache.get_next_object(4), Some(obj5.clone())); // Next after 4 is 5
        assert_eq!(cache.get_next_object(5), None); // No object after 5

        // Empty cache
        let mut empty_cache = SubgroupStreamCache::new(header.clone(), 10);
        assert_eq!(empty_cache.get_next_object(1), None);
    }

    #[test]
    fn test_subgroup_cache_get_first_object() {
        let header = create_test_header(1, 1);
        let mut cache = SubgroupStreamCache::new(header.clone(), 10);

        assert_eq!(cache.get_first_object(), None); // Empty cache

        let obj3 = create_test_subgroup_object(3, 103);
        let obj1 = create_test_subgroup_object(1, 101); // Inserted out of order
        let obj5 = create_test_subgroup_object(5, 105);

        cache.insert_object(obj3.clone(), 1000);
        cache.insert_object(obj1.clone(), 1000);
        cache.insert_object(obj5.clone(), 1000);
        
        assert_eq!(cache.get_first_object(), Some(obj1.clone()));
    }

    #[test]
    fn test_subgroup_cache_get_latest_object() {
        let header = create_test_header(1, 1);
        let mut cache = SubgroupStreamCache::new(header.clone(), 10);

        assert_eq!(cache.get_latest_object(), None); // Empty cache

        let obj3 = create_test_subgroup_object(3, 103);
        let obj1 = create_test_subgroup_object(1, 101);
        let obj5 = create_test_subgroup_object(5, 105); // Inserted out of order

        cache.insert_object(obj3.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj3.clone()));

        cache.insert_object(obj1.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj3.clone())); // Still 3, as 1 < 3

        cache.insert_object(obj5.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj5.clone())); // Now 5 is the latest
    }

    #[test]
    fn test_subgroup_cache_get_largest_object_id() {
        let header = create_test_header(1, 1);
        let mut cache = SubgroupStreamCache::new(header.clone(), 10);

        assert_eq!(cache.get_largest_object_id(), None); // Empty cache

        let obj3 = create_test_subgroup_object(3, 103);
        let obj1 = create_test_subgroup_object(1, 101);
        let obj5 = create_test_subgroup_object(5, 105);

        cache.insert_object(obj3.clone(), 1000);
        assert_eq!(cache.get_largest_object_id(), Some(3));

        cache.insert_object(obj1.clone(), 1000);
        assert_eq!(cache.get_largest_object_id(), Some(3));

        cache.insert_object(obj5.clone(), 1000);
        assert_eq!(cache.get_largest_object_id(), Some(5));
    }
}
