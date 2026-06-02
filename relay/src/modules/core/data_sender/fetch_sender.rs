#[async_trait::async_trait]
pub(crate) trait FetchSender: 'static + Send + Sync {
    async fn send(&self, object: moqt::FetchObjectField) -> anyhow::Result<()>;
    async fn close(&self) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> FetchSender for moqt::FetchDataSender<T> {
    async fn send(&self, object: moqt::FetchObjectField) -> anyhow::Result<()> {
        self.send(object).await
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.close().await
    }
}
