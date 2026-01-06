#[async_trait::async_trait]
pub(crate) trait DataSender: 'static + Send + Sync {
    async fn send(&self, object: moqt::DataObject) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<S: moqt::SendStreamType> DataSender for moqt::DataSender<S> {
    async fn send(&self, object: moqt::DataObject) -> anyhow::Result<()> {
        self.send(object).await
    }
}
