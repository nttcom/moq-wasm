use dashmap::DashMap;

use crate::modules::relay::relay::Relay;

pub struct RelayManager {
    pub(crate) relay_map: DashMap<u128, Relay>,
}

impl RelayManager {
    pub(crate) fn new() -> Self {
        Self {
            relay_map: DashMap::new(),
        }
    }
}