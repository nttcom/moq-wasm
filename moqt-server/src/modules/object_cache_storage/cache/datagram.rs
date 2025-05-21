use moqt_core::messages::data_streams::DatagramObject;
use std::time::Duration;
use ttl_cache::TtlCache;

#[derive(Clone)]
pub(crate) struct DatagramCache {
    objects: TtlCache<(u64, u64), DatagramObject>,
}

impl DatagramCache {
    pub(crate) fn new(max_store_size: usize) -> Self {
        let objects = TtlCache::new(max_store_size);

        Self { objects }
    }

    pub(crate) fn insert_object(&mut self, object: DatagramObject, duration: u64) {
        let ttl = Duration::from_millis(duration);
        let group_id = match &object {
            DatagramObject::ObjectDatagram(obj) => obj.group_id(),
            DatagramObject::ObjectDatagramStatus(obj) => obj.group_id(),
        };
        let object_id = match &object {
            DatagramObject::ObjectDatagram(obj) => obj.object_id(),
            DatagramObject::ObjectDatagramStatus(obj) => obj.object_id(),
        };
        self.objects.insert((group_id, object_id), object, ttl);
    }

    pub(crate) fn get_object(
        &mut self,
        group_id: u64,
        object_id: u64,
    ) -> Option<DatagramObject> {
        self.objects.get(&(group_id, object_id)).cloned()
    }

    pub(crate) fn get_next_object(
        &mut self,
        group_id: u64,
        current_object_id: u64,
    ) -> Option<DatagramObject> {
        self.objects
            .iter()
            .filter_map(|((g_id, o_id), obj)| {
                if *g_id == group_id && *o_id > current_object_id {
                    Some((*o_id, obj.clone()))
                } else {
                    None
                }
            })
            .min_by_key(|(o_id, _)| *o_id)
            .map(|(_, obj)| obj)
    }

    pub(crate) fn get_latest_group(&mut self) -> Option<DatagramObject> {
        let latest_group_id = self.objects.iter().map(|((g_id, _), _)| g_id).max();

        match latest_group_id {
            Some(lg_id) => self
                .objects
                .iter()
                .filter_map(|((g_id, o_id), obj)| {
                    if g_id == lg_id {
                        Some((*o_id, obj.clone()))
                    } else {
                        None
                    }
                })
                .min_by_key(|(o_id, _)| *o_id)
                .map(|(_, obj)| obj),
            None => None,
        }
    }

    pub(crate) fn get_latest_object(&mut self) -> Option<DatagramObject> {
        self.objects
            .iter()
            .max_by_key(|((g_id, o_id), _)| (*g_id, *o_id))
            .map(|(_, v)| v.clone())
    }

    pub(crate) fn get_largest_group_id(&mut self) -> Option<u64> {
        self.objects.iter().map(|((g_id, _), _)| *g_id).max()
    }

    pub(crate) fn get_largest_object_id(&mut self) -> Option<u64> {
        let largest_group_id = match self.get_largest_group_id() {
            Some(id) => id,
            None => return None,
        };

        self.objects
            .iter()
            .filter_map(|((g_id, o_id), _)| {
                if *g_id == largest_group_id {
                    Some(*o_id)
                } else {
                    None
                }
            })
            .max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moqt_core::messages::data_streams::datagram;

    fn create_test_datagram_object(group_id: u64, object_id: u64, payload_byte: u8) -> DatagramObject {
        DatagramObject::ObjectDatagram(
            datagram::Object::new(
                0, // track_alias
                group_id,
                object_id,
                0, // publisher_priority
                vec![], // extension_headers
                vec![payload_byte], // object_payload
            )
            .unwrap(),
        )
    }

    #[test]
    fn test_insert_and_get_object() {
        let mut cache = DatagramCache::new(10);
        let obj1_g1 = create_test_datagram_object(1, 1, 101);
        let obj2_g1 = create_test_datagram_object(1, 2, 102);
        let obj1_g2 = create_test_datagram_object(2, 1, 201);

        cache.insert_object(obj1_g1.clone(), 1000);
        cache.insert_object(obj2_g1.clone(), 1000);
        cache.insert_object(obj1_g2.clone(), 1000);

        // Test getting existing objects
        assert_eq!(cache.get_object(1, 1), Some(obj1_g1.clone()));
        assert_eq!(cache.get_object(1, 2), Some(obj2_g1.clone()));
        assert_eq!(cache.get_object(2, 1), Some(obj1_g2.clone()));

        // Test getting non-existent objects
        assert_eq!(cache.get_object(1, 3), None); // Non-existent object_id in group 1
        assert_eq!(cache.get_object(3, 1), None); // Non-existent group_id
        assert_eq!(cache.get_object(2, 2), None); // Non-existent object_id in group 2
    }

    #[test]
    fn test_get_next_object() {
        let mut cache = DatagramCache::new(10);
        let obj1_g1 = create_test_datagram_object(1, 1, 101);
        let obj2_g1 = create_test_datagram_object(1, 3, 103); // Note: object_id 3, not 2
        let obj3_g1 = create_test_datagram_object(1, 5, 105);
        let obj1_g2 = create_test_datagram_object(2, 1, 201);

        cache.insert_object(obj1_g1.clone(), 1000);
        cache.insert_object(obj2_g1.clone(), 1000);
        cache.insert_object(obj3_g1.clone(), 1000);
        cache.insert_object(obj1_g2.clone(), 1000);

        // Next object in the same group
        assert_eq!(cache.get_next_object(1, 1), Some(obj2_g1.clone()));
        assert_eq!(cache.get_next_object(1, 3), Some(obj3_g1.clone()));

        // No next object in the same group (current is last)
        assert_eq!(cache.get_next_object(1, 5), None);

        // No next object if current_object_id doesn't exist but is within range
        assert_eq!(cache.get_next_object(1, 2), Some(obj2_g1.clone())); 
        assert_eq!(cache.get_next_object(1, 4), Some(obj3_g1.clone()));


        // No objects in the group or group doesn't exist
        assert_eq!(cache.get_next_object(3, 1), None);

        // Next object when other groups exist
        assert_eq!(cache.get_next_object(2, 0), Some(obj1_g2.clone()));
        assert_eq!(cache.get_next_object(2, 1), None);
    }

    #[test]
    fn test_get_latest_group() {
        let mut cache = DatagramCache::new(10);
        
        // Empty cache
        assert_eq!(cache.get_latest_group(), None);

        let obj1_g1 = create_test_datagram_object(1, 1, 101);
        let obj2_g1 = create_test_datagram_object(1, 5, 105); // larger object_id
        cache.insert_object(obj1_g1.clone(), 1000);
        cache.insert_object(obj2_g1.clone(), 1000);
        // Expected: obj1_g1 (smallest object_id in group 1)
        assert_eq!(cache.get_latest_group(), Some(obj1_g1.clone()));


        let obj1_g2 = create_test_datagram_object(2, 2, 202);
        let obj2_g2 = create_test_datagram_object(2, 3, 203);
        cache.insert_object(obj1_g2.clone(), 1000);
        cache.insert_object(obj2_g2.clone(), 1000);
        // Expected: obj1_g2 (smallest object_id in group 2, which is the latest group)
        assert_eq!(cache.get_latest_group(), Some(obj1_g2.clone()));

        let obj1_g0 = create_test_datagram_object(0, 10, 10); // Earlier group
        cache.insert_object(obj1_g0.clone(), 1000);
        assert_eq!(cache.get_latest_group(), Some(obj1_g2.clone())); // Still obj1_g2 from group 2

        // Add to a new, even later group
        let obj1_g3 = create_test_datagram_object(3, 1, 301);
        cache.insert_object(obj1_g3.clone(), 1000);
        assert_eq!(cache.get_latest_group(), Some(obj1_g3.clone()));
    }

    #[test]
    fn test_get_latest_object() {
        let mut cache = DatagramCache::new(10);

        // Empty cache
        assert_eq!(cache.get_latest_object(), None);

        let obj1_g1 = create_test_datagram_object(1, 1, 101);
        cache.insert_object(obj1_g1.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj1_g1.clone()));

        let obj2_g1 = create_test_datagram_object(1, 5, 105); // Same group, larger object_id
        cache.insert_object(obj2_g1.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj2_g1.clone()));

        let obj1_g2 = create_test_datagram_object(2, 2, 202); // New group, smaller object_id but larger group_id
        cache.insert_object(obj1_g2.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj1_g2.clone()));
        
        let obj2_g2 = create_test_datagram_object(2, 6, 206); // Same new group, larger object_id
        cache.insert_object(obj2_g2.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj2_g2.clone()));

        // Object in an earlier group, but with a very large object_id
        let obj3_g1 = create_test_datagram_object(1, 100, 199);
        cache.insert_object(obj3_g1.clone(), 1000);
        assert_eq!(cache.get_latest_object(), Some(obj2_g2.clone())); // Still obj2_g2 because group_id 2 > 1
    }
}
