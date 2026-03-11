use std::fmt::Debug;

use async_trait::async_trait;

use crate::modules::{
    core::subscription::Subscription,
    enums::{ContentExists, FilterType, GroupOrder},
};

pub(crate) struct SubscribeOption {
    pub(crate) subscriber_priority: u8,
    pub(crate) group_order: GroupOrder,
    pub(crate) forward: bool,
    pub(crate) filter_type: FilterType,
}

#[async_trait]
pub(crate) trait PublishHandler: 'static + Send + Sync + Debug {
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
    fn track_alias(&self) -> u64;
    fn _group_order(&self) -> GroupOrder;
    fn _content_exists(&self) -> ContentExists;
    fn _forward(&self) -> bool;
    fn _authorization_token(&self) -> Option<String>;
    fn _delivery_timeout(&self) -> Option<u64>;
    fn _max_cache_duration(&self) -> Option<u64>;
    async fn ok(&self, subscriber_priority: u8, filter_type: FilterType) -> anyhow::Result<()>;
    async fn _error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()>;
    fn convert_into_subscription(&self, expires: u64) -> Subscription;
}

#[async_trait]
impl<T: moqt::TransportProtocol> PublishHandler for moqt::PublishHandler<T> {
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    fn track_name(&self) -> &str {
        &self.track_name
    }
    fn track_alias(&self) -> u64 {
        self.track_alias
    }
    fn _group_order(&self) -> GroupOrder {
        GroupOrder::from(self.group_order)
    }
    fn _content_exists(&self) -> ContentExists {
        ContentExists::from(self.content_exists)
    }
    fn _forward(&self) -> bool {
        self.forward
    }
    fn _authorization_token(&self) -> Option<String> {
        self.authorization_token.clone()
    }
    fn _delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }
    fn _max_cache_duration(&self) -> Option<u64> {
        self.max_cache_duration
    }

    async fn ok(&self, subscriber_priority: u8, filter_type: FilterType) -> anyhow::Result<()> {
        self.ok(subscriber_priority, filter_type.as_moqt()).await
    }

    async fn _error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()> {
        self.error(code, reason_phrase).await
    }

    fn convert_into_subscription(&self, expires: u64) -> Subscription {
        Subscription::from(self.into_subscription(expires))
    }
}
