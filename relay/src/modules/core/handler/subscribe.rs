use async_trait::async_trait;

use crate::modules::{
    core::published_resource::PublishedResource,
    enums::{ContentExists, FilterType, GroupOrder},
};

#[async_trait]
pub(crate) trait SubscribeHandler: 'static + Send + Sync {
    fn subscribe_id(&self) -> u64;
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
    fn _subscriber_priority(&self) -> u8;
    fn _group_order(&self) -> GroupOrder;
    fn _forward(&self) -> bool;
    fn _filter_type(&self) -> FilterType;
    fn _authorization_token(&self) -> Option<String>;
    fn _max_cache_duration(&self) -> Option<u64>;
    fn _delivery_timeout(&self) -> Option<u64>;
    async fn ok(&self, expires: u64, content_exists: ContentExists) -> anyhow::Result<u64>;
    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()>;
    fn convert_into_publication(&self, track_alias: u64) -> PublishedResource;
}

#[async_trait]
impl<T: moqt::TransportProtocol> SubscribeHandler for moqt::SubscribeHandler<T> {
    fn subscribe_id(&self) -> u64 {
        self.request_id()
    }
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    fn track_name(&self) -> &str {
        &self.track_name
    }
    fn _subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }
    fn _group_order(&self) -> GroupOrder {
        GroupOrder::from(self.group_order)
    }
    fn _forward(&self) -> bool {
        self.forward
    }
    fn _filter_type(&self) -> FilterType {
        FilterType::from(self.filter_type)
    }
    fn _authorization_token(&self) -> Option<String> {
        self.authorization_token.clone()
    }
    fn _max_cache_duration(&self) -> Option<u64> {
        self.max_cache_duration
    }
    fn _delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }

    async fn ok(&self, expires: u64, content_exists: ContentExists) -> anyhow::Result<u64> {
        self.ok(expires, content_exists.as_moqt()).await
    }

    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()> {
        self.error(code, reason_phrase).await
    }

    fn convert_into_publication(&self, track_alias: u64) -> PublishedResource {
        PublishedResource::from(self.into_publication(track_alias))
    }
}
