use async_trait::async_trait;

use crate::modules::{
    core::published_resource::PublishedResource,
    enums::{ContentExists, FilterType, GroupOrder},
};

#[async_trait]
pub(crate) trait SubscribeHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
    fn subscriber_priority(&self) -> u8;
    fn group_order(&self) -> GroupOrder;
    fn forward(&self) -> bool;
    fn filter_type(&self) -> FilterType;
    fn authorization_token(&self) -> Option<String>;
    fn max_cache_duration(&self) -> Option<u64>;
    fn delivery_timeout(&self) -> Option<u64>;
    async fn ok(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
    ) -> anyhow::Result<()>;
    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()>;
    fn into_publication(&self, track_alias: u64) -> PublishedResource;
}

#[async_trait]
impl<T: moqt::TransportProtocol> SubscribeHandler for moqt::SubscribeHandler<T> {
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    fn track_name(&self) -> &str {
        &self.track_name
    }
    fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }
    fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.group_order)
    }
    fn forward(&self) -> bool {
        self.forward
    }
    fn filter_type(&self) -> FilterType {
        FilterType::from(self.filter_type)
    }
    fn authorization_token(&self) -> Option<String> {
        self.authorization_token.clone()
    }
    fn max_cache_duration(&self) -> Option<u64> {
        self.max_cache_duration
    }
    fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }

    async fn ok(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
    ) -> anyhow::Result<()> {
        self.ok(track_alias, expires, content_exists.into_moqt())
            .await
    }

    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()> {
        self.error(code, reason_phrase).await
    }

    fn into_publication(&self, track_alias: u64) -> PublishedResource {
        PublishedResource::from(self.into_publication(track_alias))
    }
}
