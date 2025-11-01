#[async_trait::async_trait]
pub(crate) trait DatagramSender: 'static + Send + Sync {
    async fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> DatagramSender for moqt::DatagramSender<T> {
    async fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()> {
        self.send(object).await
    }
}
