use dashmap::DashMap;

use crate::modules::types::TrackKey;

/// track_key ごとに stream か datagram かを記録するマップ
pub(crate) struct DeliveryTypeMap {
    map: DashMap<TrackKey, bool>, // true = stream, false = datagram
}

impl DeliveryTypeMap {
    pub(crate) fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub(crate) fn set_stream(&self, track_key: TrackKey) {
        self.map.insert(track_key, true);
    }

    pub(crate) fn set_datagram(&self, track_key: TrackKey) {
        self.map.insert(track_key, false);
    }

    /// まだ登録されていない場合は None
    pub(crate) fn is_stream(&self, track_key: TrackKey) -> Option<bool> {
        self.map.get(&track_key).map(|v| *v)
    }
}
