use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::modules::{relay::notifications::subgroup_opened::SubgroupOpened, types::TrackKey};

pub(crate) struct SubgroupOpenedNotifierMap {
    map: DashMap<TrackKey, broadcast::Sender<SubgroupOpened>>,
}

impl SubgroupOpenedNotifierMap {
    pub(crate) fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub(crate) fn get_or_create(&self, track_key: &TrackKey) -> broadcast::Sender<SubgroupOpened> {
        self.map
            .entry(track_key.clone())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
}
