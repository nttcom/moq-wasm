use async_trait::async_trait;

use crate::modules::{
    core::data_sender::DataSender,
    enums::{FilterType, GroupOrder},
};

#[async_trait]
pub(crate) trait PublishedResource: 'static + Send + Sync {
    fn group_order(&self) -> GroupOrder;
    fn filter_type(&self) -> FilterType;
    async fn new_stream(&self) -> anyhow::Result<Box<dyn DataSender>>;
    fn new_datagram(&self) -> Box<dyn DataSender>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> PublishedResource for moqt::PublishedResource<T> {
    fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.group_order)
    }

    fn filter_type(&self) -> FilterType {
        FilterType::from(self.filter_type)
    }

    async fn new_stream(&self) -> anyhow::Result<Box<dyn DataSender>> {
        let sender = self.create_stream().await?;
        Ok(Box::new(sender))
    }

    fn new_datagram(&self) -> Box<dyn DataSender> {
        let sender = self.create_datagram();
        Box::new(sender)
    }
}
