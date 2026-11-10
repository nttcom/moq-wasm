use std::{collections::HashSet, sync::Mutex};

use crate::modules::domain::{session_id::SessionId, track_key::namespace_has_prefix};

/// The namespaces this relay subscribed with SUBSCRIBE_NAMESPACE at the relays
/// publishing them. draft-18 withdraws a namespace only on the request that
/// told of it (§10.17: NAMESPACE_DONE answers a SUBSCRIBE_NAMESPACE), and a
/// namespace learned from the route registry came on no request, so this relay
/// subscribes it at its publisher relay to be told when it is withdrawn.
#[derive(Default)]
pub(crate) struct RelayNamespaceSubscriptions {
    subscribed: Mutex<HashSet<(SessionId, String)>>,
}

impl RelayNamespaceSubscriptions {
    /// Returns false when a subscription of the session already covers the
    /// namespace: draft-18 §10.18 refuses an overlapping one.
    pub(crate) fn insert(&self, session_id: SessionId, track_namespace: &str) -> bool {
        let mut subscribed = self.lock();
        let covered = subscribed.iter().any(|(subscriber, held)| {
            *subscriber == session_id && namespace_has_prefix(track_namespace, held)
        });
        !covered && subscribed.insert((session_id, track_namespace.to_string()))
    }

    /// Returns whether the session held the subscription.
    pub(crate) fn remove(&self, session_id: SessionId, track_namespace: &str) -> bool {
        self.lock()
            .remove(&(session_id, track_namespace.to_string()))
    }

    /// Removes and returns the subscriptions under the prefix whose namespace
    /// `still_wanted` rejects.
    pub(crate) fn remove_under_prefix(
        &self,
        track_namespace_prefix: &str,
        still_wanted: impl Fn(&str) -> bool,
    ) -> Vec<(SessionId, String)> {
        let mut subscribed = self.lock();
        let released: Vec<_> = subscribed
            .iter()
            .filter(|(_, track_namespace)| {
                namespace_has_prefix(track_namespace, track_namespace_prefix)
                    && !still_wanted(track_namespace)
            })
            .cloned()
            .collect();
        for subscription in &released {
            subscribed.remove(subscription);
        }
        released
    }

    pub(crate) fn forget_session(&self, session_id: SessionId) {
        self.lock()
            .retain(|(subscriber, _)| *subscriber != session_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<(SessionId, String)>> {
        self.subscribed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY_A: SessionId = 1;
    const RELAY_B: SessionId = 2;

    #[test]
    fn a_namespace_is_subscribed_once_per_session() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        subscriptions.insert(RELAY_A, "room/alice");

        // Act / Assert
        assert!(!subscriptions.insert(RELAY_A, "room/alice"));
        assert!(subscriptions.insert(RELAY_B, "room/alice"));
    }

    #[test]
    fn a_namespace_under_a_subscribed_one_is_not_subscribed_again() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        subscriptions.insert(RELAY_A, "room/alice");

        // Act / Assert
        assert!(!subscriptions.insert(RELAY_A, "room/alice/screen"));
        assert!(subscriptions.insert(RELAY_A, "room/alice2"));
    }

    #[test]
    fn removing_under_a_prefix_keeps_the_wanted_and_the_unrelated() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        for track_namespace in ["room/alice", "room/bob", "lobby/carol"] {
            subscriptions.insert(RELAY_A, track_namespace);
        }

        // Act
        let released = subscriptions
            .remove_under_prefix("room", |track_namespace| track_namespace == "room/bob");

        // Assert: only room/alice goes; room/bob is still wanted and lobby/carol lies elsewhere
        assert_eq!(released, vec![(RELAY_A, "room/alice".to_string())]);
        assert!(!subscriptions.remove(RELAY_A, "room/alice"));
        assert!(subscriptions.remove(RELAY_A, "room/bob"));
        assert!(subscriptions.remove(RELAY_A, "lobby/carol"));
    }

    #[test]
    fn a_closed_session_drops_its_subscriptions_only() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        subscriptions.insert(RELAY_A, "room/alice");
        subscriptions.insert(RELAY_B, "room/alice");

        // Act
        subscriptions.forget_session(RELAY_A);

        // Assert
        assert!(!subscriptions.remove(RELAY_A, "room/alice"));
        assert!(subscriptions.remove(RELAY_B, "room/alice"));
    }
}
