use std::fmt::Debug;

use async_trait::async_trait;

#[async_trait]
pub(crate) trait DataReceiver: 'static + Send + Sync + Debug {
    fn track_alias(&self) -> u64;
    fn is_datagram(&self) -> bool;
    async fn receive_object(&mut self) -> anyhow::Result<moqt::DataObject>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> DataReceiver for moqt::DataReceiver<T> {
    fn track_alias(&self) -> u64 {
        self.track_alias()
    }
    
    fn is_datagram(&self) -> bool {
        self.is_datagram()
    }

    async fn receive_object(&mut self) -> anyhow::Result<moqt::DataObject> {
        let datagram = self.receive().await?;
        Ok(datagram)
    }
}
