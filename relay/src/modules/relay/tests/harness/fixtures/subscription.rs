use crate::modules::core::subscription::DownstreamSubscription;

pub(crate) fn make_subscription(filter_type: moqt::FilterType) -> DownstreamSubscription {
    DownstreamSubscription::from(moqt::Subscription::SubscriberInitiated(
        moqt::SubscriberInitiatedSubscription {
            request_id: 0,
            track_namespace: "ns".to_string(),
            track_name: "track".to_string(),
            track_alias: 0,
            expires: 0,
            group_order: moqt::GroupOrder::Ascending,
            content_exists: moqt::ContentExists::False,
            filter_type,
            delivery_timeout: None,
        },
    ))
}
