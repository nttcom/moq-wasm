use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::SessionEvent;
use crate::modules::extensions::mutex::lock;

/// A namespace the peer published (`active`) or withdrew
/// (PUBLISH_NAMESPACE / PUBLISH_NAMESPACE_DONE, draft-14 §9.23, §9.26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceEvent {
    pub namespace: Vec<String>,
    pub active: bool,
}

#[derive(Clone, Default)]
pub(crate) struct NamespaceWatchers {
    watchers: Arc<Mutex<Vec<Arc<Watcher>>>>,
}

struct Watcher {
    prefix: Vec<String>,
    queue: Mutex<CoalescingQueue>,
    pushed: Notify,
}

impl NamespaceWatchers {
    /// Call this before sending the SUBSCRIBE_NAMESPACE: the peer publishes the
    /// namespaces it knows right away. The receiver ends once the session
    /// disconnects, `close` is called, or the receiver itself is closed.
    pub(crate) fn watch(&self, prefix: Vec<String>) -> NamespaceEventReceiver {
        let watcher = Arc::new(Watcher {
            prefix,
            queue: Mutex::new(CoalescingQueue::default()),
            pushed: Notify::new(),
        });
        lock(&self.watchers).push(watcher.clone());
        NamespaceEventReceiver {
            watchers: self.clone(),
            watcher,
        }
    }

    /// Acknowledges PUBLISH_NAMESPACE, reports it and PUBLISH_NAMESPACE_DONE to the
    /// watchers, and returns every other event. On `Disconnected` it first ends
    /// every watcher.
    pub(crate) async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        match event {
            SessionEvent::PublishNamespace(handler) => {
                tracing::info!(track_namespace = %handler.track_namespace, "PUBLISH_NAMESPACE received");
                if let Err(error) = handler.ok().await {
                    tracing::warn!(track_namespace = %handler.track_namespace, %error, "failed to acknowledge PUBLISH_NAMESPACE");
                }
                self.notify(handler.track_namespace_tuple, true);
                None
            }
            SessionEvent::PublishNamespaceDone(handler) => {
                tracing::info!(track_namespace = %handler.track_namespace, "PUBLISH_NAMESPACE_DONE received");
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

    pub(crate) fn close(&self) {
        // `recv` returns the pending events, then `None`.
        for watcher in lock(&self.watchers).drain(..) {
            lock(&watcher.queue).closed = true;
            watcher.pushed.notify_waiters();
        }
    }

    fn notify(&self, namespace: Vec<String>, active: bool) {
        for watcher in lock(&self.watchers).iter() {
            if namespace.starts_with(&watcher.prefix) {
                lock(&watcher.queue).push(namespace.clone(), active);
                watcher.pushed.notify_waiters();
            }
        }
    }

    fn remove(&self, watcher: &Arc<Watcher>) {
        lock(&self.watchers).retain(|registered| !Arc::ptr_eq(registered, watcher));
    }
}

/// The events of one watcher, coalesced per namespace so a reader that falls
/// behind holds at most one event per namespace the peer reported.
pub(crate) struct NamespaceEventReceiver {
    watchers: NamespaceWatchers,
    watcher: Arc<Watcher>,
}

impl NamespaceEventReceiver {
    /// The next namespace whose state changed, with its latest state; `None`
    /// once the watcher ends. Cancel safe.
    pub(crate) async fn recv(&self) -> Option<NamespaceEvent> {
        loop {
            let pushed = self.watcher.pushed.notified();
            {
                let mut queue = lock(&self.watcher.queue);
                if let Some(event) = queue.pop() {
                    return Some(event);
                }
                if queue.closed {
                    return None;
                }
            }
            pushed.await;
        }
    }

    /// Unregisters the watcher and ends `recv`, dropping the pending events.
    pub(crate) fn close(&self) {
        self.watchers.remove(&self.watcher);
        *lock(&self.watcher.queue) = CoalescingQueue {
            closed: true,
            ..CoalescingQueue::default()
        };
        self.watcher.pushed.notify_waiters();
    }
}

impl Drop for NamespaceEventReceiver {
    fn drop(&mut self) {
        self.watchers.remove(&self.watcher);
    }
}

/// Pending events in the order each namespace first became pending, holding
/// only the latest `active` per namespace.
#[derive(Default)]
struct CoalescingQueue {
    order: VecDeque<Vec<String>>,
    active: HashMap<Vec<String>, bool>,
    closed: bool,
}

impl CoalescingQueue {
    fn push(&mut self, namespace: Vec<String>, active: bool) {
        if self.closed {
            return;
        }
        if let Some(pending) = self.active.get_mut(&namespace) {
            *pending = active;
            return;
        }
        self.active.insert(namespace.clone(), active);
        self.order.push_back(namespace);
    }

    fn pop(&mut self) -> Option<NamespaceEvent> {
        let namespace = self.order.pop_front()?;
        let active = self
            .active
            .remove(&namespace)
            .expect("every namespace in `order` has its state in `active`");
        Some(NamespaceEvent { namespace, active })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::moqt::domains::namespace_tuple;
    use crate::modules::test_support::{next_event, spawn_connected_native_sessions};

    async fn handle_next(client: &crate::Session, watchers: &NamespaceWatchers) {
        let event = next_event(client).await;
        assert!(watchers.handle(event).await.is_none());
    }

    fn event(namespace: &str, active: bool) -> NamespaceEvent {
        NamespaceEvent {
            namespace: namespace_tuple(namespace),
            active,
        }
    }

    #[tokio::test]
    async fn a_published_and_withdrawn_namespace_reaches_only_the_watchers_of_its_prefix() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions("namespace-watchers").await;
        let watchers = NamespaceWatchers::default();
        let room = watchers.watch(namespace_tuple("anon/room"));
        let other = watchers.watch(namespace_tuple("anon/other"));

        // Act
        let publisher = server.publisher();
        let (published, ()) = tokio::join!(
            publisher.publish_namespace("anon/room/alice".to_string()),
            handle_next(&client, &watchers)
        );
        let first = room.recv().await;
        publisher
            .publish_namespace_done("anon/room/alice".to_string())
            .await
            .unwrap();
        handle_next(&client, &watchers).await;

        // Assert
        assert_eq!(
            [first, room.recv().await],
            [
                Some(event("anon/room/alice", true)),
                Some(event("anon/room/alice", false)),
            ]
        );
        assert!(published.is_ok());
        assert!(lock(&other.watcher.queue).pop().is_none());
    }

    #[tokio::test]
    async fn an_unread_namespace_keeps_its_place_and_its_latest_state() {
        // Arrange
        let watchers = NamespaceWatchers::default();
        let room = watchers.watch(namespace_tuple("anon/room"));

        // Act
        watchers.notify(namespace_tuple("anon/room/alice"), true);
        watchers.notify(namespace_tuple("anon/room/bob"), true);
        watchers.notify(namespace_tuple("anon/room/alice"), false);
        watchers.close();

        // Assert
        assert_eq!(
            [room.recv().await, room.recv().await],
            [
                Some(event("anon/room/alice", false)),
                Some(event("anon/room/bob", true)),
            ]
        );
    }

    #[tokio::test]
    async fn disconnected_ends_every_watcher_and_is_returned() {
        // Arrange
        let watchers = NamespaceWatchers::default();
        let room = watchers.watch(namespace_tuple("anon/room"));

        // Act
        let returned = watchers.handle(SessionEvent::Disconnected()).await;

        // Assert
        assert!(matches!(returned, Some(SessionEvent::Disconnected())));
        assert!(room.recv().await.is_none());
    }

    #[tokio::test]
    async fn close_unregisters_the_watcher_and_ends_a_waiting_recv() {
        // Arrange
        let watchers = NamespaceWatchers::default();
        let room = watchers.watch(namespace_tuple("anon/room"));

        // Act
        let (received, ()) = tokio::join!(room.recv(), async {
            tokio::task::yield_now().await;
            room.close();
        });
        watchers.notify(namespace_tuple("anon/room/alice"), true);

        // Assert
        assert_eq!(received, None);
        assert!(room.recv().await.is_none());
        assert!(lock(&watchers.watchers).is_empty());
    }

    #[tokio::test]
    async fn close_drops_the_pending_events() {
        // Arrange
        let watchers = NamespaceWatchers::default();
        let room = watchers.watch(namespace_tuple("anon/room"));
        watchers.notify(namespace_tuple("anon/room/alice"), true);

        // Act
        room.close();

        // Assert
        assert!(room.recv().await.is_none());
    }

    #[test]
    fn a_dropped_receiver_is_unregistered() {
        // Arrange
        let watchers = NamespaceWatchers::default();
        let room = watchers.watch(namespace_tuple("anon/room"));

        // Act
        drop(room);

        // Assert
        assert!(lock(&watchers.watchers).is_empty());
    }

    mod properties {
        use proptest::{collection::vec, prelude::*};

        use super::super::*;

        #[derive(Debug, Clone)]
        enum Op {
            Push(usize, bool),
            Pop,
        }

        /// Few namespaces so pushes often hit a pending one.
        fn op() -> impl Strategy<Value = Op> {
            prop_oneof![
                3 => (0..4usize, any::<bool>()).prop_map(|(index, active)| Op::Push(index, active)),
                1 => Just(Op::Pop),
            ]
        }

        fn namespace(index: usize) -> Vec<String> {
            vec!["room".to_string(), index.to_string()]
        }

        proptest! {
            #[test]
            fn queue_coalesces_like_the_model(ops in vec(op(), 0..64)) {
                // Arrange
                let mut queue = CoalescingQueue::default();
                let mut model: Vec<(Vec<String>, bool)> = Vec::new();
                let mut latest: HashMap<Vec<String>, bool> = HashMap::new();
                let mut seen: HashMap<Vec<String>, bool> = HashMap::new();

                for op in ops {
                    // Act
                    match op {
                        Op::Push(index, active) => {
                            let namespace = namespace(index);
                            queue.push(namespace.clone(), active);
                            latest.insert(namespace.clone(), active);
                            match model.iter_mut().find(|(pending, _)| *pending == namespace) {
                                Some((_, pending)) => *pending = active,
                                None => model.push((namespace, active)),
                            }
                        }
                        Op::Pop => {
                            let popped = queue.pop();
                            let expected = (!model.is_empty()).then(|| model.remove(0));
                            // Assert: pops follow first appearance with the latest state
                            prop_assert_eq!(
                                popped.clone().map(|event| (event.namespace, event.active)),
                                expected
                            );
                            if let Some(event) = popped {
                                seen.insert(event.namespace, event.active);
                            }
                        }
                    }

                    // Assert: at most one pending entry per namespace
                    prop_assert_eq!(queue.order.len(), model.len());
                    prop_assert_eq!(queue.active.len(), model.len());
                }

                // Act
                while let Some(event) = queue.pop() {
                    seen.insert(event.namespace, event.active);
                }

                // Assert: a reader that catches up sees every namespace's latest state
                prop_assert_eq!(seen, latest);
            }
        }
    }
}
