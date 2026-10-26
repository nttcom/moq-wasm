use async_trait::async_trait;
use moqt::{RequestErrorCode, namespace::NamespaceInterest};

/// A draft-18 update moving the prefix of a namespace subscription the
/// session holds (draft-18 §10.9.2); see
/// `moqt::namespace::SubscribeNamespaceUpdateHandler`.
#[async_trait]
pub(crate) trait SubscribeNamespaceUpdateHandler: 'static + Send + Sync {
    fn track_namespace_prefix(&self) -> &str;
    fn new_track_namespace_prefix(&self) -> &str;
    fn new_track_namespace_prefix_tuple(&self) -> &[String];
    fn interest(&self) -> NamespaceInterest;
    async fn ok(&self);
    async fn error(&self, code: RequestErrorCode, reason_phrase: String);
}

#[async_trait]
impl SubscribeNamespaceUpdateHandler for moqt::namespace::SubscribeNamespaceUpdateHandler {
    fn track_namespace_prefix(&self) -> &str {
        self.track_namespace_prefix()
    }

    fn new_track_namespace_prefix(&self) -> &str {
        self.new_track_namespace_prefix()
    }

    fn new_track_namespace_prefix_tuple(&self) -> &[String] {
        self.new_track_namespace_prefix_tuple()
    }

    fn interest(&self) -> NamespaceInterest {
        self.interest()
    }

    async fn ok(&self) {
        self.ok().await;
    }

    async fn error(&self, code: RequestErrorCode, reason_phrase: String) {
        self.error(code, reason_phrase).await
    }
}
