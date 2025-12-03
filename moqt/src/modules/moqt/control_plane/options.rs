use std::time::{SystemTime, UNIX_EPOCH};

use crate::modules::moqt::control_plane::messages::control_messages::{
    enums::{ContentExists, FilterType},
    group_order::GroupOrder,
};

pub struct PublishOption {
    pub track_alias: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: ContentExists,
    pub(crate) forward: bool,
}

impl Default for PublishOption {
    fn default() -> Self {
        Self {
            track_alias: Self::get_track_alias(),
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            forward: true,
        }
    }
}

impl PublishOption {
    fn get_track_alias() -> u64 {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_nanos() as u64;
        tracing::debug!("track alias: {}", id);
        id
    }
}

pub struct SubscribeOption {
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    pub forward: bool,
    pub filter_type: FilterType,
}

impl Default for SubscribeOption {
    fn default() -> Self {
        Self {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LatestObject,
        }
    }
}
