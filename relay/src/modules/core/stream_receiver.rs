use std::fmt::Debug;

use async_trait::async_trait;

#[async_trait]
pub(crate) trait StreamReceiver: 'static + Send + Sync + Debug {
    // fn track_alias(&self) -> u64;
    // async fn receive(&self) -> anyhow::Result<moqt::DatagramObject>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> StreamReceiver for moqt::StreamReceiver<T> {
    // fn track_alias(&self) -> u64 {
    //     self.track_alias
    // }

    // async fn receive(&self) -> anyhow::Result<moqt::DatagramObject> {
    //     let data = self.receive().await?;

    // }
}
