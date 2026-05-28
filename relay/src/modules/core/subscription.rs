use crate::modules::enums::{ContentExists, FilterType};

#[derive(Clone)]
pub struct UpstreamSubscription {
    inner: moqt::Subscription,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct DownstreamSubscription {
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

#[allow(dead_code)]
impl DownstreamSubscription {
    pub(crate) fn as_moqt(&self) -> &moqt::Subscription {
        &self.inner
    }
}

impl From<moqt::Subscription> for DownstreamSubscription {
    fn from(inner: moqt::Subscription) -> Self {
        Self { inner }
    }
}
