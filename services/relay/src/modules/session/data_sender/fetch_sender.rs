use moqt::{fetch::UnknownRange, object::DataStreamResetCode};
#[async_trait::async_trait]
pub(crate) trait FetchSender: 'static + Send + Sync {
    async fn send(&self, object: moqt::object::Object) -> anyhow::Result<()>;
    /// See `moqt::fetch::FetchDataSender::report_unknown_range`.
    async fn report_unknown_range(
        &self,
        last: moqt::object::Location,
    ) -> anyhow::Result<UnknownRange>;
    async fn close(&self) -> anyhow::Result<()>;
    async fn reset(&self, error_code: DataStreamResetCode) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl FetchSender for moqt::fetch::FetchDataSender {
    async fn send(&self, object: moqt::object::Object) -> anyhow::Result<()> {
        self.send_object(&object).await
    }

    async fn report_unknown_range(
        &self,
        last: moqt::object::Location,
    ) -> anyhow::Result<UnknownRange> {
        moqt::fetch::FetchDataSender::report_unknown_range(self, last).await
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.close().await
    }

    async fn reset(&self, error_code: DataStreamResetCode) -> anyhow::Result<()> {
        self.reset(error_code).await
    }
}
