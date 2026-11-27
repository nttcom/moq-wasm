use std::sync::Arc;

use crate::{
    Session, modules::moqt::domains::namespace_watchers::NamespaceEventReceiver,
    namespace::NamespaceEvent,
};

/// The namespaces published and withdrawn under one SUBSCRIBE_NAMESPACE
/// prefix (draft-14 §9.28), until the session ends or `close`. A reader that
/// falls behind gets one event per changed namespace with its latest state.
pub struct NamespaceWatch {
    session: Arc<Session>,
    prefix: String,
    events: NamespaceEventReceiver,
}

impl NamespaceWatch {
    pub(super) fn new(
        session: Arc<Session>,
        prefix: String,
        events: NamespaceEventReceiver,
    ) -> Self {
        Self {
            session,
            prefix,
            events,
        }
    }

    /// The next namespace published or withdrawn; `None` once the session
    /// ends or `close` is called.
    pub async fn next(&self) -> Option<NamespaceEvent> {
        self.events.recv().await
    }

    /// UNSUBSCRIBE_NAMESPACE (draft-14 §9.31); a `next` that waits returns
    /// `None`.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.events.close();
        self.session
            .subscriber()
            .unsubscribe_namespace(self.prefix.clone())
            .await
    }
}
