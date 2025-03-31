use super::CacheId;
use moqt_core::messages::data_streams::DatagramObject;
use std::time::Duration;
use ttl_cache::TtlCache;

#[derive(Clone)]
pub(crate) struct DatagramCache {
    objects: TtlCache<CacheId, DatagramObject>,
    next_cache_id: CacheId,
}

impl DatagramCache {
    pub(crate) fn new(max_store_size: usize) -> Self {
        let objects = TtlCache::new(max_store_size);

        Self {
            objects,
            next_cache_id: 0,
        }
    }

    pub(crate) fn insert_object(&mut self, object: DatagramObject, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(self.next_cache_id, object, ttl);
        self.next_cache_id += 1;
    }

    pub(crate) fn get_object(
        &mut self,
        group_id: u64,
        object_id: u64,
    ) -> Option<(CacheId, DatagramObject)> {
        self.objects.iter().find_map(|(k, v)| {
            let g_id = match v {
                DatagramObject::ObjectDatagram(obj) => obj.group_id(),
                DatagramObject::ObjectDatagramStatus(obj) => obj.group_id(),
            };

            let o_id = match v {
                DatagramObject::ObjectDatagram(obj) => obj.object_id(),
                DatagramObject::ObjectDatagramStatus(obj) => obj.object_id(),
            };

            if g_id == group_id && o_id == object_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    pub(crate) fn get_next_object(
        &mut self,
        cache_id: CacheId,
    ) -> Option<(CacheId, DatagramObject)> {
        let next_cache_id = cache_id + 1;
        self.objects.iter().find_map(|(k, v)| {
            if *k == next_cache_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    pub(crate) fn get_latest_group(&mut self) -> Option<(CacheId, DatagramObject)> {
        let latest_group_id = self
            .objects
            .iter()
            .last()
            .map(|(_, v)| match v {
                DatagramObject::ObjectDatagram(obj) => obj.group_id(),
                DatagramObject::ObjectDatagramStatus(obj) => obj.group_id(),
            })
            .unwrap();

        let latest_group = self.objects.iter().filter_map(|(k, v)| {
            let g_id = match v {
                DatagramObject::ObjectDatagram(obj) => obj.group_id(),
                DatagramObject::ObjectDatagramStatus(obj) => obj.group_id(),
            };
            if g_id == latest_group_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        });

        latest_group.min_by_key(|(k, v)| {
            let o_id = match v {
                DatagramObject::ObjectDatagram(obj) => obj.object_id(),
                DatagramObject::ObjectDatagramStatus(obj) => obj.object_id(),
            };
            (o_id, *k)
        })
    }

    pub(crate) fn get_latest_object(&mut self) -> Option<(CacheId, DatagramObject)> {
        self.objects.iter().last().map(|(k, v)| (*k, v.clone()))
    }

    pub(crate) fn get_largest_group_id(&mut self) -> Option<u64> {
        self.objects
            .iter()
            .map(|(_, v)| match v {
                DatagramObject::ObjectDatagram(obj) => obj.group_id(),
                DatagramObject::ObjectDatagramStatus(obj) => obj.group_id(),
            })
            .max()
    }

    pub(crate) fn get_largest_object_id(&mut self) -> Option<u64> {
        let largest_group_id = self.get_largest_group_id()?;

        self.objects
            .iter()
            .filter_map(|(_, v)| {
                let g_id = match v {
                    DatagramObject::ObjectDatagram(obj) => obj.group_id(),
                    DatagramObject::ObjectDatagramStatus(obj) => obj.group_id(),
                };

                let o_id = match v {
                    DatagramObject::ObjectDatagram(obj) => obj.object_id(),
                    DatagramObject::ObjectDatagramStatus(obj) => obj.object_id(),
                };

                if g_id == largest_group_id {
                    Some(o_id)
                } else {
                    None
                }
            })
            .max()
    }
}
