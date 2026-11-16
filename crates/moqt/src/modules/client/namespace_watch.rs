use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, watch};

use crate::{Session, namespace::NamespaceEvent};

/// The namespaces published and withdrawn under one SUBSCRIBE_NAMESPACE
/// prefix (draft-14 §9.28), until the session ends or `close`.
pub struct NamespaceWatch {
    session: Arc<Session>,
    prefix: String,
    events: Mutex<mpsc::UnboundedReceiver<NamespaceEvent>>,
    /// Ends a `next` that waits, since it holds `events` meanwhile.
    closed: watch::Sender<bool>,
}

impl NamespaceWatch {
    pub(super) fn new(
        session: Arc<Session>,
        prefix: String,
        events: mpsc::UnboundedReceiver<NamespaceEvent>,
    ) -> Self {
        Self {
            session,
            prefix,
            events: Mutex::new(events),
            closed: watch::channel(false).0,
        }
    }

    /// The next namespace published or withdrawn; `None` once the session
    /// ends or `close` is called.
    pub async fn next(&self) -> Option<NamespaceEvent> {
        let mut closed = self.closed.subscribe();
        let mut events = self.events.lock().await;
        tokio::select! {
            biased;
            _ = closed.wait_for(|closed| *closed) => None,
            event = events.recv() => event,
        }
    }

    /// UNSUBSCRIBE_NAMESPACE (draft-14 §9.31); a `next` that waits returns
    /// `None`.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.closed.send_replace(true);
        self.events.lock().await.close();
        self.session
            .subscriber()
            .unsubscribe_namespace(self.prefix.clone())
            .await
    }
}
