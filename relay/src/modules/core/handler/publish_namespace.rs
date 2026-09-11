use async_trait::async_trait;

#[async_trait]
pub(crate) trait PublishNamespaceHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
    fn track_namespace_tuple(&self) -> &[String];
    async fn ok(&self) -> Result<(), moqt::TransportSendError>;
    async fn error(&self, code: u64, reason_phrase: String)
    -> Result<(), moqt::TransportSendError>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> PublishNamespaceHandler for moqt::PublishNamespaceHandler<T> {
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        self.ok().await
    }

    async fn error(
        &self,
        code: u64,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.error(code, reason_phrase).await
    }
}
