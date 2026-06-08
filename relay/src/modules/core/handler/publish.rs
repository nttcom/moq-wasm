use std::fmt::Debug;

use async_trait::async_trait;

use crate::modules::{
    core::subscription::UpstreamSubscription,
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
    fn subscription(
        &self,
        subscriber_priority: u8,
        filter_type: FilterType,
    ) -> UpstreamSubscription;
    async fn accept_data_receiver(&self);
    async fn ok(&self, subscription: &UpstreamSubscription)
    -> Result<(), moqt::TransportSendError>;
    async fn error(&self, code: u64, reason_phrase: String)
    -> Result<(), moqt::TransportSendError>;
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

    fn subscription(
        &self,
        subscriber_priority: u8,
        filter_type: FilterType,
    ) -> UpstreamSubscription {
        UpstreamSubscription::from(moqt::PublisherInitiatedSubscription {
            request_id: self.request_id,
            track_namespace: self.track_namespace.clone(),
            track_name: self.track_name.clone(),
            track_alias: self.track_alias,
            group_order: self.group_order,
            content_exists: self.content_exists,
            subscriber_priority,
            forward: self.forward,
            filter_type: filter_type.as_moqt(),
            delivery_timeout: self.delivery_timeout,
        })
    }

    async fn ok(
        &self,
        subscription: &UpstreamSubscription,
    ) -> Result<(), moqt::TransportSendError> {
        let Some((subscriber_priority, filter_type)) = subscription.publish_accept_options() else {
            tracing::error!("PUBLISH_OK requires publisher-initiated upstream subscription");
            return Ok(());
        };
        self.ok(subscriber_priority, filter_type.as_moqt(), 0)
            .await
            .map(|_| ())
    }

    async fn accept_data_receiver(&self) {
        self.accept_data_receiver().await;
    }

    async fn error(
        &self,
        code: u64,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.error(code, reason_phrase).await
    }
}
