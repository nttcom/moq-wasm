use std::time::{SystemTime, UNIX_EPOCH};

use crate::modules::moqt::messages::control_messages::{
    enums::FilterType, group_order::GroupOrder, location::Location,
};

pub struct PublishOption {
    pub track_alias: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: bool,
    pub(crate) largest_location: Option<Location>,
    pub(crate) forward: bool,
}

impl Default for PublishOption {
    fn default() -> Self {
        Self {
            track_alias: Self::get_track_alias(),
            group_order: GroupOrder::Ascending,
            content_exists: false,
            largest_location: None,
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
    pub start_location: Option<Location>,
    pub end_group: Option<u64>,
}

impl Default for SubscribeOption {
    fn default() -> Self {
        Self {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LatestObject,
            start_location: None,
            end_group: None,
        }
    }
}
