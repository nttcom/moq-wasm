use async_trait::async_trait;

use crate::modules::{
    core::publication::Publication,
    enums::{FilterType, Location},
};

#[async_trait]
pub(crate) trait SubscribeHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
    fn subscriber_priority(&self) -> u8;
    fn group_order(&self) -> moqt::GroupOrder;
    fn forward(&self) -> bool;
    fn filter_type(&self) -> FilterType;
    fn start_location(&self) -> Option<Location>;
    fn end_group(&self) -> Option<u64>;
    fn authorization_token(&self) -> Option<String>;
    fn max_cache_duration(&self) -> Option<u64>;
    fn delivery_timeout(&self) -> Option<u64>;
    async fn ok(&self) -> anyhow::Result<()>;
    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()>;
    fn into_publication(&self, track_alias: u64) -> Box<dyn Publication>;
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
    fn group_order(&self) -> moqt::GroupOrder {
        self.group_order
    }
    fn forward(&self) -> bool {
        self.forward
    }
    fn filter_type(&self) -> FilterType {
        match self.filter_type {
            moqt::FilterType::LatestGroup => FilterType::LatestGroup,
            moqt::FilterType::LatestObject => FilterType::LatestObject,
            moqt::FilterType::AbsoluteStart => FilterType::AbsoluteStart,
            moqt::FilterType::AbsoluteRange => FilterType::AbsoluteRange,
        }
    }
    fn start_location(&self) -> Option<Location> {
        if let Some(largest_location) = self.start_location {
            Some(Location {
                object_id: largest_location.object_id,
                group_id: largest_location.group_id,
            })
        } else {
            None
        }
    }
    fn end_group(&self) -> Option<u64> {
        self.end_group
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

    async fn ok(&self) -> anyhow::Result<()> {
        self.ok().await
    }

    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()> {
        self.error(code, reason_phrase).await
    }

    fn into_publication(&self, track_alias: u64) -> Box<dyn Publication> {
        let publication = self.into_publication(track_alias);
        Box::new(publication)
    }
}
