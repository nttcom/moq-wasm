use std::{
    collections::BTreeMap,
    sync::{Arc, PoisonError, RwLock},
};

use relay_stats::RelaySnapshot;

#[derive(Clone, Default)]
pub struct LatestSnapshots {
    by_relay: Arc<RwLock<BTreeMap<String, RelaySnapshot>>>,
}

impl LatestSnapshots {
    pub fn update(&self, snapshot: RelaySnapshot) {
        self.by_relay
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(snapshot.relay_id.clone(), snapshot);
    }

    pub fn all(&self) -> Vec<RelaySnapshot> {
        self.by_relay
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}
