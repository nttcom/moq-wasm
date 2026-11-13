use std::time::Duration;

use crate::{
    TrackProperties,
    modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists,
    object::{GroupOrder, Location},
    subscribe::FilterType,
};

#[derive(Clone)]
pub struct PublishOption {
    pub content_exists: ContentExists,
    pub forward: bool,
    /// Sent in the PUBLISH, with the track's group order (§12.5).
    pub track_properties: TrackProperties,
}

impl Default for PublishOption {
    fn default() -> Self {
        Self {
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
#[derive(Clone, Copy, Debug)]
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
    /// Sent as NEW_GROUP_REQUEST (draft-18 §10.2.13) to ask the Original
    /// Publisher for a new group: the largest Group ID the subscriber knows
    /// plus 1, or 0 when it knows none. A track without dynamic groups
    /// ignores it. Draft-14 has no such parameter and sends none.
    pub new_group_request: Option<u64>,
    /// DELIVERY TIMEOUT in milliseconds (draft-14 §9.2.1.2, draft-18 §10.2.4
    /// OBJECT_DELIVERY_TIMEOUT): how long after receiving an object the
    /// publisher keeps trying to deliver it. `None` sends no parameter, and
    /// every object is delivered.
    pub delivery_timeout: Option<u64>,
}

impl Default for SubscribeOption {
    fn default() -> Self {
        Self {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LargestObject,
            new_group_request: None,
            delivery_timeout: None,
        }
    }
}
