use async_trait::async_trait;
use moqt::{RequestErrorCode, namespace::NamespaceInterest};

use crate::logging::mark_current_span_failed;

#[async_trait]
pub(crate) trait SubscribeNamespaceHandler: 'static + Send + Sync {
    fn trace_context(&self) -> Option<&moqt::TraceContext>;
    fn track_namespace_prefix(&self) -> &str;
    fn track_namespace_prefix_tuple(&self) -> &[String];
    fn interest(&self) -> NamespaceInterest;
    /// The Forward State of the PUBLISH messages the subscription receives.
    fn forward(&self) -> bool;
    async fn ok(&self) -> Result<(), moqt::TransportSendError>;
    async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError>;
}

#[async_trait]
impl SubscribeNamespaceHandler for moqt::namespace::SubscribeNamespaceHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        moqt::namespace::SubscribeNamespaceHandler::trace_context(self)
    }
    fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }

    fn track_namespace_prefix_tuple(&self) -> &[String] {
        &self.track_namespace_prefix_tuple
    }

    fn interest(&self) -> NamespaceInterest {
        self.interest
    }

    fn forward(&self) -> bool {
        self.forward
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
