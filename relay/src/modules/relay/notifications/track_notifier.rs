use dashmap::DashMap;

use crate::modules::{relay::notifications::track_event::TrackEvent, types::TrackKey};

pub(crate) struct ObjectNotifyProducerMap {
    map: DashMap<TrackKey, tokio::sync::broadcast::Sender<TrackEvent>>,
}

impl ObjectNotifyProducerMap {
    pub(crate) fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub(crate) fn get_or_create(
        &self,
        track_key: TrackKey,
    ) -> tokio::sync::broadcast::Sender<TrackEvent> {
        let _ = self
            .map
            .entry(track_key)
            .or_insert_with(|| tokio::sync::broadcast::channel(256).0);
        self.map.get(&track_key).unwrap().clone()
    }
}
