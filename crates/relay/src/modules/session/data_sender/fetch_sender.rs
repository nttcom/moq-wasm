use moqt::object::DataStreamResetCode;
#[async_trait::async_trait]
pub(crate) trait FetchSender: 'static + Send + Sync {
    async fn send(&self, object: moqt::fetch::FetchObjectField) -> anyhow::Result<()>;
    async fn close(&self) -> anyhow::Result<()>;
    async fn reset(&self, error_code: DataStreamResetCode) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl FetchSender for moqt::fetch::FetchDataSender {
    async fn send(&self, object: moqt::fetch::FetchObjectField) -> anyhow::Result<()> {
        self.send(object).await
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.close().await
    }

    async fn reset(&self, error_code: DataStreamResetCode) -> anyhow::Result<()> {
        self.reset(error_code).await
    }
}
