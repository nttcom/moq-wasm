use std::fmt::Debug;

use async_trait::async_trait;

#[async_trait]
pub(crate) trait DatagramReceiver: 'static + Send + Sync + Debug {
    fn track_alias(&self) -> u64;
    async fn receive(&self) -> anyhow::Result<moqt::ObjectDatagram>;
}

#[async_trait]
impl DatagramReceiver for moqt::DatagramReceiver {
    fn track_alias(&self) -> u64 {
        self.track_alias
    }

    async fn receive(&self) -> anyhow::Result<moqt::ObjectDatagram> {
        self.receive().await
    }
}
