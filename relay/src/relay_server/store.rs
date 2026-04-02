use std::sync::Arc;

use crate::modules::relay::{
    cache::store::TrackCacheStore,
    notifications::{delivery_type_map::DeliveryTypeMap, sender_map::SenderMap},
};

pub(crate) struct RelayStore {
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) sender_map: Arc<SenderMap>,
    pub(crate) delivery_type_map: Arc<DeliveryTypeMap>,
}

impl RelayStore {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cache_store: Arc::new(TrackCacheStore::new()),
            sender_map: Arc::new(SenderMap::new()),
            delivery_type_map: Arc::new(DeliveryTypeMap::new()),
        })
    }
}
