use std::time::Duration;

use crate::{
    TrackProperties,
    modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists,
    object::{GroupOrder, Location},
    subscribe::FilterType,
};

#[derive(Clone)]
pub struct PublishOption {
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub forward: bool,
    pub track_properties: TrackProperties,
}

impl Default for PublishOption {
    fn default() -> Self {
        Self {
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            forward: true,
            track_properties: TrackProperties::default(),
        }
    }
}

pub struct FetchOption {
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    /// Sent as FILL_TIMEOUT (draft-18 §10.2.5): the most a relay should wait
    /// for objects it must fetch upstream before it reports them as unknown,
    /// `ZERO` asking for only what it holds. Millisecond precision; draft-14
    /// has no such parameter and sends none.
    pub fill_timeout: Option<Duration>,
}

impl Default for FetchOption {
    fn default() -> Self {
        Self {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            fill_timeout: None,
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

#[derive(Clone)]
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
