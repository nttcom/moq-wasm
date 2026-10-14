use crate::modules::session::subscription::{DownstreamSubscription, UpstreamSubscription};

fn subscriber_initiated(
    request_id: u64,
    filter_type: moqt::subscribe::FilterType,
) -> moqt::Subscription {
    moqt::Subscription::SubscriberInitiated(moqt::SubscriberInitiatedSubscription {
        request_id,
        track_namespace: "ns".to_string(),
        track_name: "track".to_string(),
        track_alias: 0,
        expires: 0,
        group_order: moqt::object::GroupOrder::Ascending,
        subscriber_priority: 128,
        content_exists: moqt::object::ContentExists::False,
        filter_type,
        delivery_timeout: None,
        max_cache_duration: None,
    })
}

pub(crate) fn make_subscription(
    filter_type: moqt::subscribe::FilterType,
) -> DownstreamSubscription {
    DownstreamSubscription::from(subscriber_initiated(0, filter_type))
}

/// An upstream subscription identified by `request_id`, for directory entries
/// whose tests only care which request is released.
pub(crate) fn upstream_subscription(request_id: u64) -> UpstreamSubscription {
    UpstreamSubscription::from(subscriber_initiated(
        request_id,
        moqt::subscribe::FilterType::LargestObject,
    ))
}
