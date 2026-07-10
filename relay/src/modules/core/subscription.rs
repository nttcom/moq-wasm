use crate::modules::enums::{ContentExists, FilterType, GroupOrder};

#[derive(Clone)]
pub struct UpstreamSubscription {
    inner: moqt::Subscription,
}

/// A subscription the relay serves toward a downstream subscriber. Mirrors
/// `UpstreamSubscription`: it holds the moqt `Subscription` and the relay sends
/// objects on it. Receiving a SUBSCRIBE makes it subscriber-initiated; the relay
/// forwarding a PUBLISH makes it publisher-initiated.
#[derive(Clone)]
pub(crate) struct DownstreamSubscription {
    inner: moqt::Subscription,
}

impl UpstreamSubscription {
    pub(crate) fn as_moqt(&self) -> &moqt::Subscription {
        &self.inner
    }

    pub(crate) fn request_id(&self) -> u64 {
        self.inner.request_id()
    }

    pub(crate) fn track_alias(&self) -> u64 {
        self.inner.track_alias()
    }

    pub(crate) fn track_namespace(&self) -> &str {
        self.inner.track_namespace()
    }

    pub(crate) fn track_name(&self) -> &str {
        self.inner.track_name()
    }

    pub(crate) fn expires(&self) -> Option<u64> {
        self.inner.expires()
    }

    pub(crate) fn content_exists(&self) -> ContentExists {
        ContentExists::from(self.inner.content_exists())
    }

    pub(crate) fn publish_accept_options(&self) -> Option<(u8, FilterType)> {
        match &self.inner {
            moqt::Subscription::PublisherInitiated(subscription) => Some((
                subscription.subscriber_priority,
                FilterType::from(subscription.filter_type),
            )),
            moqt::Subscription::SubscriberInitiated(_) => None,
        }
    }
}

impl From<moqt::Subscription> for UpstreamSubscription {
    fn from(inner: moqt::Subscription) -> Self {
        Self { inner }
    }
}

impl From<moqt::PublisherInitiatedSubscription> for UpstreamSubscription {
    fn from(subscription: moqt::PublisherInitiatedSubscription) -> Self {
        Self {
            inner: moqt::Subscription::PublisherInitiated(subscription),
        }
    }
}

impl DownstreamSubscription {
    pub(crate) fn as_moqt(&self) -> &moqt::Subscription {
        &self.inner
    }

    pub(crate) fn track_alias(&self) -> u64 {
        self.inner.track_alias()
    }

    pub(crate) fn filter_type(&self) -> FilterType {
        FilterType::from(self.inner.filter_type())
    }

    pub(crate) fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.inner.group_order())
    }
}

impl From<moqt::Subscription> for DownstreamSubscription {
    fn from(inner: moqt::Subscription) -> Self {
        Self { inner }
    }
}
