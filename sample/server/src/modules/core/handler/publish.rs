use async_trait::async_trait;

use crate::modules::{enums::Location, types::GroupOrder};

#[async_trait]
pub(crate) trait PublishHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
    fn track_alias(&self) -> u64;
    fn group_order(&self) -> GroupOrder;
    fn content_exists(&self) -> bool;
    fn largest_location(&self) -> Option<Location>;
    fn forward(&self) -> bool;
    fn authorization_token(&self) -> Option<String>;
    fn delivery_timeout(&self) -> Option<u64>;
    fn max_cache_duration(&self) -> Option<u64>;
    async fn ok(&self) -> anyhow::Result<()>;
    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()>;
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
    fn group_order(&self) -> GroupOrder {
        self.group_order
    }
    fn content_exists(&self) -> bool {
        self.content_exists
    }
    fn largest_location(&self) -> Option<Location> {
        if let Some(largest_location) = self.largest_location {
            Some(Location {
                object_id: largest_location.object_id,
                group_id: largest_location.group_id,
            })
        } else {
            None
        }
    }
    fn forward(&self) -> bool {
        self.forward
    }
    fn authorization_token(&self) -> Option<String> {
        self.authorization_token.clone()
    }
    fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }
    fn max_cache_duration(&self) -> Option<u64> {
        self.max_cache_duration
    }

    async fn ok(&self) -> anyhow::Result<()> {
        self.ok().await
    }

    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()> {
        self.error(code, reason_phrase).await
    }
}
