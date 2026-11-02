use async_trait::async_trait;
use moqt::RequestErrorCode;

use crate::logging::mark_current_span_failed;

#[async_trait]
pub(crate) trait SubscribeNamespaceHandler: 'static + Send + Sync {
    fn track_namespace_prefix(&self) -> &str;
    fn track_namespace_prefix_tuple(&self) -> &[String];
    async fn ok(&self) -> Result<(), moqt::TransportSendError>;
    async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError>;
}

#[async_trait]
impl SubscribeNamespaceHandler for moqt::namespace::SubscribeNamespaceHandler {
    fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }

    fn track_namespace_prefix_tuple(&self) -> &[String] {
        &self.track_namespace_prefix_tuple
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        self.ok().await
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        mark_current_span_failed(format!("{code:?}"));
        self.error(code, reason_phrase).await
    }
}
