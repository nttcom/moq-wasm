use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::SessionEvent;

/// A namespace the peer published (`active`) or withdrew
/// (PUBLISH_NAMESPACE / PUBLISH_NAMESPACE_DONE, draft-14 §9.23, §9.26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceEvent {
    pub namespace: Vec<String>,
    pub active: bool,
}

/// Hands the namespaces a peer publishes to every watcher of a matching
/// prefix. The session's event loop passes each event to `handle`. Register a
/// watcher with `watch` before sending its SUBSCRIBE_NAMESPACE, since the
/// peer publishes the namespaces it knows right away.
#[derive(Clone, Default)]
pub struct NamespaceWatchers {
    watchers: Arc<Mutex<Vec<Watcher>>>,
}

/// A prefix and where its events go.
type Watcher = (Vec<String>, mpsc::UnboundedSender<NamespaceEvent>);

impl NamespaceWatchers {
    /// The receiver ends once the session disconnects or `close` is called.
    pub fn watch(&self, prefix: Vec<String>) -> mpsc::UnboundedReceiver<NamespaceEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.lock().push((prefix, sender));
        receiver
    }

    /// Acknowledges PUBLISH_NAMESPACE and reports it and
    /// PUBLISH_NAMESPACE_DONE to the watchers; returns every other event.
    /// `Disconnected` is returned too, after it ended every watcher.
    pub async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        match event {
            SessionEvent::PublishNamespace(handler) => {
                if let Err(error) = handler.ok().await {
                    tracing::warn!(track_namespace = %handler.track_namespace, %error, "failed to acknowledge PUBLISH_NAMESPACE");
                }
                self.notify(handler.track_namespace_tuple, true);
                None
            }
            SessionEvent::PublishNamespaceDone(handler) => {
                self.notify(handler.track_namespace_tuple, false);
                None
            }
            event @ SessionEvent::Disconnected() => {
                self.close();
                Some(event)
            }
            event => Some(event),
        }
    }

    pub fn close(&self) {
        self.lock().clear();
    }

    fn notify(&self, namespace: Vec<String>, active: bool) {
        let event = NamespaceEvent { namespace, active };
        self.lock().retain(|(prefix, sender)| {
            !event.namespace.starts_with(prefix) || sender.send(event.clone()).is_ok()
        });
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, Vec<(Vec<String>, mpsc::UnboundedSender<NamespaceEvent>)>> {
        self.watchers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::{spawn_connected_native_sessions, within};

    fn namespace(path: &str) -> Vec<String> {
        path.split('/').map(str::to_string).collect()
    }

    async fn handle_next(client: &crate::Session, watchers: &NamespaceWatchers) {
        let event = within(client.receive_event()).await.unwrap();
        assert!(watchers.handle(event).await.is_none());
    }

    #[tokio::test]
    async fn a_published_and_withdrawn_namespace_reaches_only_the_watchers_of_its_prefix() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions("namespace-watchers").await;
        let watchers = NamespaceWatchers::default();
        let mut room = watchers.watch(namespace("anon/room"));
        let mut other = watchers.watch(namespace("anon/other"));

        // Act
        let publisher = server.publisher();
        let (published, ()) = tokio::join!(
            publisher.publish_namespace("anon/room/alice".to_string()),
            handle_next(&client, &watchers)
        );
        publisher
            .publish_namespace_done("anon/room/alice".to_string())
            .await
            .unwrap();
        handle_next(&client, &watchers).await;

        // Assert
        assert_eq!(
            [room.recv().await.unwrap(), room.recv().await.unwrap()],
            [
                NamespaceEvent {
                    namespace: namespace("anon/room/alice"),
                    active: true,
                },
                NamespaceEvent {
                    namespace: namespace("anon/room/alice"),
                    active: false,
                },
            ]
        );
        assert!(published.is_ok());
        assert!(other.try_recv().is_err());
    }

    #[tokio::test]
    async fn disconnected_ends_every_watcher_and_is_returned() {
        // Arrange
        let watchers = NamespaceWatchers::default();
        let mut room = watchers.watch(namespace("anon/room"));

        // Act
        let returned = watchers.handle(SessionEvent::Disconnected()).await;

        // Assert
        assert!(matches!(returned, Some(SessionEvent::Disconnected())));
        assert!(room.recv().await.is_none());
    }
}
