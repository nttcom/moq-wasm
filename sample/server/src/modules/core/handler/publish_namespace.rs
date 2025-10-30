use async_trait::async_trait;

#[async_trait]
pub(crate) trait PublishNamespaceHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
    async fn ok(&self) -> anyhow::Result<()>;
    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> PublishNamespaceHandler for moqt::PublishNamespaceHandler<T> {
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    async fn ok(&self) -> anyhow::Result<()> {
        self.ok().await
    }

    async fn error(&self, code: u64, reason_phrase: String) -> anyhow::Result<()> {
        self.error(code, reason_phrase).await
    }
}
