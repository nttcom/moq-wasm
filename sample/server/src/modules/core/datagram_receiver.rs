use async_trait::async_trait;

#[async_trait]
pub(crate) trait DatagramReceiver: 'static + Send + Sync {
    async fn receive(&self) -> anyhow::Result<moqt::DatagramObject>;
}

#[async_trait]
impl DatagramReceiver for moqt::DatagramReceiver {
    async fn receive(&self) -> anyhow::Result<moqt::DatagramObject> {
        self.receive().await
    }
}
