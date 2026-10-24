use bytes::Bytes;
use moqt::object::{KeyValuePair, VariantType};

use crate::modules::session::subscription::{DownstreamSubscription, UpstreamSubscription};

/// Track Properties the relay does not understand (draft-18 §2.5): Immutable
/// Properties whose contents keep a two-byte encoding of 5 (§12.7), and an
/// application property carrying `mark`.
pub(crate) fn unknown_track_properties(mark: u64) -> moqt::TrackProperties {
    moqt::TrackProperties {
        max_cache_duration: None,
        other: vec![
            KeyValuePair {
                key: 0x0B,
                value: VariantType::Odd(Bytes::from_static(&[0x38, 0x80, 0x05])),
            },
            KeyValuePair {
                key: 0x38,
                value: VariantType::Even(mark),
            },
        ],
    }
}

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
        track_properties: moqt::TrackProperties::default(),
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

pub(crate) fn upstream_subscription_with_track_properties(
    request_id: u64,
    track_properties: moqt::TrackProperties,
) -> UpstreamSubscription {
    let mut subscription =
        subscriber_initiated(request_id, moqt::subscribe::FilterType::LargestObject);
    if let moqt::Subscription::SubscriberInitiated(subscriber_initiated) = &mut subscription {
        subscriber_initiated.track_properties = track_properties;
    }
    UpstreamSubscription::from(subscription)
}
