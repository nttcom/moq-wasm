use async_trait::async_trait;
use moqt::wire::AuthorizationToken;

#[async_trait]
pub(crate) trait TrackStatusHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
    fn authorization_tokens(&self) -> &[AuthorizationToken];
    async fn ok(&self) -> Result<(), moqt::TransportSendError>;
    async fn error(&self, code: u64, reason_phrase: String)
    -> Result<(), moqt::TransportSendError>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> TrackStatusHandler for moqt::TrackStatusHandler<T> {
    fn request_id(&self) -> u64 {
        self.request_id()
    }

    fn track_namespace(&self) -> &str {
        self.track_namespace()
    }

    fn track_name(&self) -> &str {
        self.track_name()
    }

    fn authorization_tokens(&self) -> &[AuthorizationToken] {
        self.authorization_tokens()
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
