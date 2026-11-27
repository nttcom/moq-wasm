use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use crate::modules::domain::{session_id::SessionId, track_key::namespace_has_prefix};

/// The namespaces this relay subscribed with SUBSCRIBE_NAMESPACE at the relays
/// publishing them. draft-18 withdraws a namespace only on the request that
/// told of it (§10.17: NAMESPACE_DONE answers a SUBSCRIBE_NAMESPACE), and a
/// namespace learned from the route registry came on no request, so this relay
/// subscribes it at its publisher relay to be told when it is withdrawn.
#[derive(Default)]
pub(crate) struct RelayNamespaceSubscriptions {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    subscribed: HashSet<(SessionId, String)>,
    /// (session, Request ID) of PUBLISH_NAMESPACEs on replaced subscriptions;
    /// their PUBLISH_NAMESPACE_DONE is ignored.
    superseded: HashSet<(SessionId, u64)>,
    /// Per session, the subscriptions replaced by one not answered yet.
    replacing: HashMap<SessionId, Replacement>,
    session_locks: HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
}

struct Replacement {
    prefixes: Vec<String>,
    /// Namespaces under `prefixes` withdrawn while the replacement is
    /// answered; the caller decides after the answer whether clients lose them.
    withdrawn: Vec<String>,
}

impl RelayNamespaceSubscriptions {
    /// Serializes subscribing, replacing and ending the subscriptions of the
    /// session: hold the guard from reading or changing them until the
    /// SUBSCRIBE_NAMESPACE or UNSUBSCRIBE_NAMESPACE is answered or sent, so
    /// the publisher relay never gets one overlapping a subscription it holds
    /// (draft-18 §10.18).
    pub(crate) async fn lock_session(
        &self,
        session_id: SessionId,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .lock()
            .session_locks
            .entry(session_id)
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    /// Holds the namespace for the session and returns the narrower
    /// subscriptions of the session it replaces, which the caller ends before
    /// subscribing it: draft-18 §10.18 refuses an overlapping one. Returns
    /// `None` when a subscription of the session already covers it. Call
    /// under `lock_session`, and `finish_replacement` after the answer when
    /// some were replaced.
    pub(crate) fn insert(
        &self,
        session_id: SessionId,
        track_namespace: &str,
    ) -> Option<Vec<String>> {
        let mut state = self.lock();
        let covered = state.subscribed.iter().any(|(subscriber, held)| {
            *subscriber == session_id && namespace_has_prefix(track_namespace, held)
        });
        if covered {
            return None;
        }
        let replaced: Vec<String> = state
            .subscribed
            .iter()
            .filter(|(subscriber, held)| {
                *subscriber == session_id && namespace_has_prefix(held, track_namespace)
            })
            .map(|(_, held)| held.clone())
            .collect();
        for held in &replaced {
            state.subscribed.remove(&(session_id, held.clone()));
        }
        state
            .subscribed
            .insert((session_id, track_namespace.to_string()));
        if !replaced.is_empty() {
            state.replacing.insert(
                session_id,
                Replacement {
                    prefixes: replaced.clone(),
                    withdrawn: Vec::new(),
                },
            );
        }
        Some(replaced)
    }

    /// Returns whether the session held the subscription.
    pub(crate) fn remove(&self, session_id: SessionId, track_namespace: &str) -> bool {
        self.lock()
            .subscribed
            .remove(&(session_id, track_namespace.to_string()))
    }

    /// Sessions holding a subscription under `track_namespace_prefix`.
    pub(crate) fn sessions_under_prefix(&self, track_namespace_prefix: &str) -> HashSet<SessionId> {
        self.lock()
            .subscribed
            .iter()
            .filter(|(_, track_namespace)| {
                namespace_has_prefix(track_namespace, track_namespace_prefix)
            })
            .map(|(session_id, _)| *session_id)
            .collect()
    }

    /// Removes the session's subscriptions under `track_namespace_prefix` that
    /// are no longer `still_wanted`, returning them.
    pub(crate) fn remove_under_prefix(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        still_wanted: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut state = self.lock();
        let released: Vec<String> = state
            .subscribed
            .iter()
            .filter(|(subscriber, track_namespace)| {
                *subscriber == session_id
                    && namespace_has_prefix(track_namespace, track_namespace_prefix)
                    && !still_wanted(track_namespace)
            })
            .map(|(_, track_namespace)| track_namespace.clone())
            .collect();
        for track_namespace in &released {
            state
                .subscribed
                .remove(&(session_id, track_namespace.clone()));
        }
        released
    }

    /// Makes `take_superseded` report these PUBLISH_NAMESPACEs, so their DONE
    /// is ignored. Call before ending the replaced subscription.
    pub(crate) fn supersede(
        &self,
        session_id: SessionId,
        request_ids: impl IntoIterator<Item = u64>,
    ) {
        self.lock().superseded.extend(
            request_ids
                .into_iter()
                .map(|request_id| (session_id, request_id)),
        );
    }

    pub(crate) fn take_superseded(&self, session_id: SessionId, request_id: u64) -> bool {
        self.lock().superseded.remove(&(session_id, request_id))
    }

    /// Returns true, recording the namespace for `finish_replacement`, when
    /// it lies under a subscription of the session being replaced: the
    /// withdrawal may only end that subscription's request.
    pub(crate) fn defer_withdrawal(&self, session_id: SessionId, track_namespace: &str) -> bool {
        let mut state = self.lock();
        let Some(replacement) = state.replacing.get_mut(&session_id) else {
            return false;
        };
        let replaced = replacement
            .prefixes
            .iter()
            .any(|prefix| namespace_has_prefix(track_namespace, prefix));
        if replaced {
            replacement.withdrawn.push(track_namespace.to_string());
        }
        replaced
    }

    /// Ends the session's replacement and returns the namespaces whose
    /// withdrawal `defer_withdrawal` held back.
    pub(crate) fn finish_replacement(&self, session_id: SessionId) -> Vec<String> {
        self.lock()
            .replacing
            .remove(&session_id)
            .map(|replacement| replacement.withdrawn)
            .unwrap_or_default()
    }

    pub(crate) fn forget_session(&self, session_id: SessionId) {
        let mut state = self.lock();
        state
            .subscribed
            .retain(|(subscriber, _)| *subscriber != session_id);
        state
            .superseded
            .retain(|(subscriber, _)| *subscriber != session_id);
        state.replacing.remove(&session_id);
        state.session_locks.remove(&session_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
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
        assert_eq!(subscriptions.insert(RELAY_A, "room/alice"), None);
        assert_eq!(subscriptions.insert(RELAY_B, "room/alice"), Some(vec![]));
    }

    #[test]
    fn a_namespace_under_a_subscribed_one_is_not_subscribed_again() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        subscriptions.insert(RELAY_A, "room/alice");

        // Act / Assert
        assert_eq!(subscriptions.insert(RELAY_A, "room/alice/screen"), None);
        assert_eq!(subscriptions.insert(RELAY_A, "room/alice2"), Some(vec![]));
    }

    #[test]
    fn a_namespace_over_subscribed_ones_replaces_them_on_that_session() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        for track_namespace in ["room/alice/screen", "room/alice/camera", "room/alice2"] {
            subscriptions.insert(RELAY_A, track_namespace);
        }
        subscriptions.insert(RELAY_B, "room/alice/screen");

        // Act
        let mut replaced = subscriptions.insert(RELAY_A, "room/alice").unwrap();

        // Assert: room/alice2 lies outside room/alice; RELAY_B keeps its own
        replaced.sort();
        assert_eq!(replaced, ["room/alice/camera", "room/alice/screen"]);
        assert!(!subscriptions.remove(RELAY_A, "room/alice/screen"));
        assert!(subscriptions.remove(RELAY_A, "room/alice"));
        assert!(subscriptions.remove(RELAY_A, "room/alice2"));
        assert!(subscriptions.remove(RELAY_B, "room/alice/screen"));
    }

    #[test]
    fn a_superseded_publication_is_taken_once_and_forgotten_with_its_session() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        subscriptions.supersede(RELAY_A, [3, 5]);
        subscriptions.supersede(RELAY_B, [3]);

        // Act
        let taken = subscriptions.take_superseded(RELAY_A, 3);
        subscriptions.forget_session(RELAY_A);

        // Assert
        assert!(taken);
        assert!(!subscriptions.take_superseded(RELAY_A, 3));
        assert!(!subscriptions.take_superseded(RELAY_A, 5));
        assert!(subscriptions.take_superseded(RELAY_B, 3));
    }

    #[test]
    fn withdrawals_under_replaced_subscriptions_are_deferred_until_the_replacement_finishes() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        subscriptions.insert(RELAY_A, "room/alice/screen");
        subscriptions.insert(RELAY_A, "room/alice");

        // Act
        let deferred = ["room/alice/screen/hd", "room/alice/camera", "room/bob"]
            .map(|track_namespace| subscriptions.defer_withdrawal(RELAY_A, track_namespace));
        let other_session = subscriptions.defer_withdrawal(RELAY_B, "room/alice/screen/hd");
        let withdrawn = subscriptions.finish_replacement(RELAY_A);

        // Assert: only room/alice/screen was replaced; nothing is deferred once it finishes
        assert_eq!(deferred, [true, false, false]);
        assert!(!other_session);
        assert_eq!(withdrawn, ["room/alice/screen/hd"]);
        assert!(!subscriptions.defer_withdrawal(RELAY_A, "room/alice/screen/hd"));
    }

    #[test]
    fn removing_under_a_prefix_keeps_the_wanted_and_the_unrelated() {
        // Arrange
        let subscriptions = RelayNamespaceSubscriptions::default();
        for track_namespace in ["room/alice", "room/bob", "lobby/carol"] {
            subscriptions.insert(RELAY_A, track_namespace);
        }

        // Act
        let released = subscriptions.remove_under_prefix(RELAY_A, "room", |track_namespace| {
            track_namespace == "room/bob"
        });

        // Assert: only room/alice goes; room/bob is still wanted and lobby/carol lies elsewhere
        assert_eq!(released, vec!["room/alice".to_string()]);
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
