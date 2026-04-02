use dashmap::DashMap;

use crate::modules::types::TrackKey;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeliveryType {
    Stream,
    Datagram,
}

/// track_key ごとに stream か datagram かを記録するマップ
pub(crate) struct DeliveryTypeMap {
    map: DashMap<TrackKey, DeliveryType>,
}

impl DeliveryTypeMap {
    pub(crate) fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub(crate) fn set_stream(&self, track_key: TrackKey) {
        self.map.insert(track_key, DeliveryType::Stream);
    }

    pub(crate) fn set_datagram(&self, track_key: TrackKey) {
        self.map.insert(track_key, DeliveryType::Datagram);
    }

    /// まだ登録されていない場合は None
    pub(crate) fn delivery_type(&self, track_key: TrackKey) -> Option<DeliveryType> {
        self.map.get(&track_key).map(|v| *v)
    }
}
