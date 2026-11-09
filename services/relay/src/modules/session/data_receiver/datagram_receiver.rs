#[async_trait::async_trait]
pub(crate) trait DatagramReceiver: Send + Sync + 'static {
    async fn receive_object(&mut self) -> anyhow::Result<moqt::object::Object>;
}

#[async_trait::async_trait]
impl DatagramReceiver for moqt::object::DatagramReceiver {
    async fn receive_object(&mut self) -> anyhow::Result<moqt::object::Object> {
        self.receive().await
    }
}
