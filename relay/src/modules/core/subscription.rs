use async_trait::async_trait;

use crate::modules::{
    core::data_receiver::{
        DataReceiver, datagram_receiver::DatagramReceiver, stream_receiver::StreamReceiver,
    },
    enums::{ContentExists, GroupOrder},
};

#[async_trait]
pub trait Subscription: 'static + Send + Sync {
    fn track_alias(&self) -> u64;
    fn expires(&self) -> u64;
    fn group_order(&self) -> GroupOrder;
    fn content_exists(&self) -> ContentExists;
    async fn create_data_receiver(&self) -> anyhow::Result<Box<dyn DataReceiver>>;
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
    async fn create_data_receiver(&self) -> anyhow::Result<Box<dyn DataReceiver>> {
        let result = self.accept_data_receiver().await?;
        match result {
            moqt::DataReceiver::Stream(inner) => {
                let receiver = StreamReceiver::new(inner);
                Ok(Box::new(receiver))
            }
            moqt::DataReceiver::Datagram(inner) => {
                let receiver = DatagramReceiver::new(inner);
                Ok(Box::new(receiver))
            }
        }
    }
}
