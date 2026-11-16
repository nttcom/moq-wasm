use std::sync::Arc;

use crate::Session;

/// A namespace `Session::announce` published.
pub struct Announcement {
    session: Arc<Session>,
    namespace: String,
}

impl Announcement {
    pub(super) fn new(session: Arc<Session>, namespace: String) -> Self {
        Self { session, namespace }
    }

    /// PUBLISH_NAMESPACE_DONE (draft-14 §9.26).
    pub async fn close(&self) -> anyhow::Result<()> {
        self.session
            .publisher()
            .publish_namespace_done(self.namespace.clone())
            .await
    }
}
