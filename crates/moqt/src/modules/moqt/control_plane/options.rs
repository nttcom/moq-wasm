use crate::{
    FilterType, GroupOrder, Location,
    modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists,
};

pub struct PublishOption {
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub forward: bool,
}

impl Default for PublishOption {
    fn default() -> Self {
        Self {
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            forward: true,
        }
    }
}

pub struct FetchOption {
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
}

impl Default for FetchOption {
    fn default() -> Self {
        Self {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
        }
    }
}

/// draft-14 §9.10: every field is sent, so a caller that only changes one
/// of them repeats the subscription's current values for the others.
pub struct SubscribeUpdateOption {
    pub start_location: Location,
    pub end_group: u64,
    pub subscriber_priority: u8,
    pub forward: bool,
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
            filter_type: FilterType::LargestObject,
        }
    }
}
