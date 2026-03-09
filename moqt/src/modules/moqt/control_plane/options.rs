use crate::{
    FilterType, GroupOrder,
    modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists,
};

pub struct PublishOption {
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: ContentExists,
    pub(crate) forward: bool,
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
