use dashmap::DashMap;

use crate::modules::relaies::relay::Relay;

pub struct RelayManager {
    pub(crate) relay_map: DashMap<u64, Relay>,
}

impl RelayManager {
    pub(crate) fn new() -> Self {
        Self {
            relay_map: DashMap::new(),
        }
    }
}
