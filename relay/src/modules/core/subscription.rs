use async_trait::async_trait;

use crate::modules::{
    core::data_receiver::DataReceiver,
    enums::{ContentExists, GroupOrder},
};

#[async_trait]
pub trait Subscription: 'static + Send + Sync {
    fn track_alias(&self) -> u64;
    fn expires(&self) -> u64;
    fn group_order(&self) -> GroupOrder;
    fn content_exists(&self) -> ContentExists;
    async fn accept_data_receiver(&self) -> anyhow::Result<Box<dyn DataReceiver>>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Subscription for moqt::Subscription<T> {
    fn track_alias(&self) -> u64 {
        self.track_alias
    }
    fn expires(&self) -> u64 {
        self.expires
    }
    fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.group_order)
    }
    fn content_exists(&self) -> ContentExists {
        ContentExists::from(self.content_exists)
    }
    async fn accept_data_receiver(&self) -> anyhow::Result<Box<dyn DataReceiver>> {
        let result = self.accept_data_receiver().await?;
        Ok(Box::new(result))
    }
}
