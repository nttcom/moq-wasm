use async_trait::async_trait;

#[async_trait]
pub(crate) trait DatagramReceiver {
    async fn receive(&self) -> anyhow::Result<moqt::DatagramObject>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> DatagramReceiver for moqt::DatagramReceiver<T> {
    async fn receive(&self) -> anyhow::Result<moqt::DatagramObject> {
        self.receive().await
    }
}
