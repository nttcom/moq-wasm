use crate::modules::enums::{ContentExists, GroupOrder};

pub struct Subscription {
    subscription: moqt::Subscription,
}

impl Subscription {
    pub(crate) fn from(subscription: moqt::Subscription) -> Self {
        Self { subscription }
    }

    pub(crate) fn as_moqt(&self) -> &moqt::Subscription {
        &self.subscription
    }

    pub(crate) fn track_alias(&self) -> u64 {
        self.subscription.track_alias
    }
    pub(crate) fn expires(&self) -> u64 {
        self.subscription.expires
    }
    pub(crate) fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.subscription.group_order)
    }
    pub(crate) fn content_exists(&self) -> ContentExists {
        ContentExists::from(self.subscription.content_exists)
    }
}
