use anyhow::bail;
use async_trait::async_trait;

use crate::modules::{
    core::datagram_receiver::DatagramReceiver,
    enums::{ContentExists, GroupOrder},
};

#[derive(Debug)]
pub(crate) enum Acceptance {
    Datagram(Box<dyn DatagramReceiver>, moqt::DatagramObject),
}

#[async_trait]
pub trait Subscription: 'static + Send + Sync {
    fn track_alias(&self) -> u64;
    fn expires(&self) -> u64;
    fn group_order(&self) -> GroupOrder;
    fn content_exists(&self) -> ContentExists;
    async fn accept_stream_or_datagram(&self) -> anyhow::Result<Acceptance>;
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
    async fn accept_stream_or_datagram(&self) -> anyhow::Result<Acceptance> {
        let result = self.accept_stream_or_datagram().await?;
        match result {
            moqt::Acceptance::Stream(stream) => {
                tracing::info!("stream accepted");
                bail!("stream accepted, but not implemented yet")
            }
            moqt::Acceptance::Datagram(datagram, object) => {
                Ok(Acceptance::Datagram(Box::new(datagram), object))
            }
        }
    }
}
