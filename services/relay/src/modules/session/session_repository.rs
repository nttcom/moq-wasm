use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use tokio::sync::{mpsc, watch};
use tracing::Span;

use crate::modules::{
    auth::{session_expiry_task::SessionExpiryTask, verified_token::VerifiedToken},
    domain::{session_id::SessionId, session_peer::SessionPeer},
    session::{Session, publisher::Publisher, session_event::SessionEvent, subscriber::Subscriber},
};

pub(crate) struct SessionRepository {
    sessions: HashMap<SessionId, SessionEntry>,
    /// Number of `SessionPeer::Client` sessions; a draining relay waits for
    /// it to reach zero.
    client_session_count: watch::Sender<usize>,
    session_added_sender: mpsc::UnboundedSender<SessionAdded>,
}

/// Sent by `SessionRepository::add`; `SessionWorkers` spawns the session's
/// worker from it.
pub(crate) struct SessionAdded {
    pub(crate) session_id: SessionId,
    pub(crate) mailbox_receiver: mpsc::UnboundedReceiver<SessionEvent>,
}

struct SessionEntry {
    session: Arc<dyn Session>,
    span: Span,
    peer: SessionPeer,
    verified_token: Arc<VerifiedToken>,
    expiry_task: Option<SessionExpiryTask>,
    mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
    /// Namespaces this relay sent PUBLISH_NAMESPACE for on the session and
    /// has not withdrawn yet.
    announced_namespaces: HashSet<String>,
}

pub(crate) struct SessionState {
    pub(crate) session_id: SessionId,
    pub(crate) peer: SessionPeer,
    pub(crate) app_id: String,
    pub(crate) transport: moqt::TransportStats,
    pub(crate) addresses: moqt::server::TransportAddresses,
}

pub(crate) struct NewSession {
    pub(crate) session_id: SessionId,
    pub(crate) session: Box<dyn Session>,
    pub(crate) session_span: Span,
    pub(crate) peer: SessionPeer,
    pub(crate) verified_token: VerifiedToken,
}

fn expiry_task(
    session: &Arc<dyn Session>,
    verified_token: &VerifiedToken,
) -> Option<SessionExpiryTask> {
    match (verified_token.is_relay, verified_token.expires_at) {
        (false, Some(expires_at)) => {
            Some(SessionExpiryTask::run(Arc::downgrade(session), expires_at))
        }
        _ => None,
    }
}

impl SessionRepository {
    pub(crate) fn new(session_added_sender: mpsc::UnboundedSender<SessionAdded>) -> Self {
        Self {
            sessions: HashMap::new(),
            client_session_count: watch::Sender::new(0),
            session_added_sender,
        }
    }

    fn update_client_session_count(&self) {
        let count = self
            .sessions
            .values()
            .filter(|entry| entry.peer == SessionPeer::Client)
            .count();
        self.client_session_count.send_replace(count);
    }

    pub(crate) fn add(&mut self, new_session: NewSession) {
        let NewSession {
            session_id,
            session,
            session_span,
            peer,
            verified_token,
        } = new_session;
        let session: Arc<dyn Session> = Arc::from(session);
        tracing::info!(
            session_id = %session_id,
            session_peer = peer.kind(),
            app_id = %verified_token.app_id,
            "session started"
        );
        let expiry_task = expiry_task(&session, &verified_token);
        let (mailbox_sender, mailbox_receiver) = mpsc::unbounded_channel();
        self.sessions.insert(
            session_id,
            SessionEntry {
                session,
                span: session_span,
                peer,
                verified_token: Arc::new(verified_token),
                expiry_task,
                mailbox_sender,
                announced_namespaces: HashSet::new(),
            },
        );
        self.update_client_session_count();
        let session_added = SessionAdded {
            session_id,
            mailbox_receiver,
        };
        if self.session_added_sender.send(session_added).is_err() {
            tracing::error!(
                session_id,
                "session worker channel closed; session is not handled"
            );
        }
    }

    pub(crate) fn remove(&mut self, session_id: SessionId) {
        let session_removed = self.sessions.remove(&session_id).is_some();
        self.update_client_session_count();
        tracing::debug!(
            session_id = %session_id,
            session_removed,
            remaining_sessions = self.sessions.len(),
            "session removed from repository"
        );
    }

    pub(crate) fn sessions_with_peer(
        &self,
        peer: SessionPeer,
    ) -> Vec<(SessionId, Arc<dyn Session>)> {
        self.sessions
            .iter()
            .filter(|(_, entry)| entry.peer == peer)
            .map(|(session_id, entry)| (*session_id, entry.session.clone()))
            .collect()
    }

    pub(crate) fn client_session_count(&self) -> watch::Receiver<usize> {
        self.client_session_count.subscribe()
    }

    pub(crate) fn session(&self, session_id: SessionId) -> Option<Arc<dyn Session>> {
        self.sessions
            .get(&session_id)
            .map(|entry| entry.session.clone())
    }

    pub(crate) fn mailbox(
        &self,
        session_id: SessionId,
    ) -> Option<mpsc::UnboundedSender<SessionEvent>> {
        self.sessions
            .get(&session_id)
            .map(|entry| entry.mailbox_sender.clone())
    }

    pub(crate) fn session_span(&self, session_id: SessionId) -> Option<Span> {
        self.sessions
            .get(&session_id)
            .map(|entry| entry.span.clone())
    }

    pub(crate) fn has_session(&self, session_id: SessionId) -> bool {
        self.sessions.contains_key(&session_id)
    }

    pub(crate) fn verified_token(&self, session_id: SessionId) -> Option<Arc<VerifiedToken>> {
        self.sessions
            .get(&session_id)
            .map(|entry| entry.verified_token.clone())
    }

    pub(crate) fn replace_verified_token(
        &mut self,
        session_id: SessionId,
        verified_token: VerifiedToken,
    ) -> Option<Arc<VerifiedToken>> {
        let entry = self.sessions.get_mut(&session_id)?;
        entry.expiry_task = expiry_task(&entry.session, &verified_token);
        entry.verified_token = Arc::new(verified_token);
        Some(entry.verified_token.clone())
    }

    pub(crate) fn is_client_session(&self, session_id: SessionId) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|entry| entry.peer == SessionPeer::Client)
    }

    pub(crate) fn subscriber(&self, session_id: SessionId) -> Option<Box<dyn Subscriber>> {
        self.sessions
            .get(&session_id)
            .map(|entry| entry.session.as_subscriber())
    }

    pub(crate) fn publisher(&self, session_id: SessionId) -> Option<Box<dyn Publisher>> {
        self.sessions
            .get(&session_id)
            .map(|entry| entry.session.as_publisher())
    }

    /// Returns false when the session is gone or the namespace is already
    /// announced on it, i.e. no PUBLISH_NAMESPACE is to be sent.
    pub(crate) fn record_namespace_announced(
        &mut self,
        session_id: SessionId,
        track_namespace: &str,
    ) -> bool {
        self.sessions.get_mut(&session_id).is_some_and(|entry| {
            entry
                .announced_namespaces
                .insert(track_namespace.to_string())
        })
    }

    /// Returns false when the session is gone or the namespace was not
    /// announced on it, i.e. no PUBLISH_NAMESPACE_DONE is to be sent.
    pub(crate) fn record_namespace_withdrawn(
        &mut self,
        session_id: SessionId,
        track_namespace: &str,
    ) -> bool {
        self.sessions
            .get_mut(&session_id)
            .is_some_and(|entry| entry.announced_namespaces.remove(track_namespace))
    }

    /// Call after UNSUBSCRIBE_NAMESPACE; otherwise a later SUBSCRIBE_NAMESPACE
    /// for the prefix gets no PUBLISH_NAMESPACE for these namespaces.
    pub(crate) fn forget_announced_namespaces(
        &mut self,
        session_id: SessionId,
        track_namespace_prefix: &str,
    ) {
        if let Some(entry) = self.sessions.get_mut(&session_id) {
            entry
                .announced_namespaces
                .retain(|track_namespace| !track_namespace.starts_with(track_namespace_prefix));
        }
    }

    pub(crate) fn session_states(&self) -> Vec<SessionState> {
        self.sessions
            .iter()
            .map(|(session_id, entry)| SessionState {
                session_id: *session_id,
                peer: entry.peer,
                app_id: entry.verified_token.app_id.clone(),
                transport: entry.session.transport_stats(),
                addresses: entry.session.transport_addresses(),
            })
            .collect()
    }

    pub(crate) fn close_with_protocol_violation(&self, session_id: SessionId, reason: &str) {
        match self.sessions.get(&session_id) {
            Some(entry) => entry
                .session
                .close(moqt::TerminationErrorCode::ProtocolViolation, reason),
            None => tracing::debug!(session_id, "session already gone; nothing to close"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use moqt::TerminationErrorCode;

    use crate::modules::{
        auth::verified_token::VerifiedToken,
        test_support::mock_session::{
            RecordedControlMessages, session_repository_with_upstream_session,
            session_repository_with_upstream_session_token,
        },
    };

    fn expired_token(is_relay: bool) -> VerifiedToken {
        VerifiedToken {
            app_id: "APP".to_string(),
            publish: Some(vec![]),
            subscribe: Some(vec![]),
            is_relay,
            expires_at: Some(SystemTime::now() - Duration::from_secs(1)),
            relay_id: None,
        }
    }

    async fn wait_for_close(recorded: &RecordedControlMessages) -> bool {
        tokio::time::timeout(Duration::from_secs(1), async {
            while recorded.closes().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn expired_client_token_closes_the_session() {
        // Arrange / Act
        let (_repository, recorded) =
            session_repository_with_upstream_session_token(7, expired_token(false)).await;

        // Assert
        assert!(wait_for_close(&recorded).await);
        assert_eq!(
            recorded.closes(),
            vec![(
                TerminationErrorCode::ExpiredAuthToken,
                "authorization token expired".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn expired_relay_token_does_not_close_the_session() {
        // Arrange / Act
        let (_repository, recorded) =
            session_repository_with_upstream_session_token(7, expired_token(true)).await;

        // Assert
        assert!(!wait_for_close(&recorded).await);
    }

    fn token_expiring_in_an_hour() -> VerifiedToken {
        VerifiedToken {
            expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            ..expired_token(false)
        }
    }

    #[tokio::test]
    async fn replace_verified_token_restarts_the_expiry_task_at_the_new_exp() {
        // Arrange
        let (repository, recorded) =
            session_repository_with_upstream_session_token(7, token_expiring_in_an_hour()).await;

        // Act
        repository
            .lock()
            .await
            .replace_verified_token(7, expired_token(false));

        // Assert
        assert!(wait_for_close(&recorded).await);
        assert_eq!(
            recorded.closes()[0].0,
            TerminationErrorCode::ExpiredAuthToken
        );
    }

    #[tokio::test]
    async fn replace_verified_token_is_visible_to_later_lookups() {
        // Arrange
        let (repository, _recorded) =
            session_repository_with_upstream_session_token(7, token_expiring_in_an_hour()).await;
        let replacement = VerifiedToken {
            app_id: "REPLACED".to_string(),
            ..token_expiring_in_an_hour()
        };

        // Act
        let returned = repository
            .lock()
            .await
            .replace_verified_token(7, replacement.clone());

        // Assert
        assert_eq!(returned.as_deref(), Some(&replacement));
        assert_eq!(
            repository.lock().await.verified_token(7).as_deref(),
            Some(&replacement)
        );
    }

    #[tokio::test]
    async fn replace_verified_token_for_an_unknown_session_is_none() {
        // Arrange
        let (repository, _recorded) = session_repository_with_upstream_session(7).await;

        // Act
        let returned = repository
            .lock()
            .await
            .replace_verified_token(8, expired_token(false));

        // Assert
        assert!(returned.is_none());
    }

    #[tokio::test]
    async fn verified_token_is_kept_until_the_session_is_removed() {
        // Arrange
        let (repository, _recorded) = session_repository_with_upstream_session(7).await;

        // Act
        let stored = repository.lock().await.verified_token(7);
        repository.lock().await.remove(7);
        let after_remove = repository.lock().await.verified_token(7);

        // Assert
        assert_eq!(stored.as_deref(), Some(&VerifiedToken::full_access()));
        assert!(after_remove.is_none());
    }

    #[tokio::test]
    async fn close_with_protocol_violation_reaches_the_session() {
        // Arrange
        let (repository, recorded) = session_repository_with_upstream_session(7).await;
        // Act
        repository
            .lock()
            .await
            .close_with_protocol_violation(7, "invalid object status 0x2");
        // Assert
        assert_eq!(
            recorded.closes(),
            vec![(
                TerminationErrorCode::ProtocolViolation,
                "invalid object status 0x2".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn close_with_protocol_violation_ignores_a_departed_session() {
        // Arrange
        let (repository, recorded) = session_repository_with_upstream_session(7).await;
        // Act
        repository
            .lock()
            .await
            .close_with_protocol_violation(8, "late");
        // Assert
        assert!(recorded.closes().is_empty());
    }

    #[tokio::test]
    async fn namespaces_forgotten_under_a_prefix_can_be_announced_again() {
        // Arrange
        let (repository, _recorded) = session_repository_with_upstream_session(7).await;
        let mut repository = repository.lock().await;
        repository.record_namespace_announced(7, "room/alice");
        repository.record_namespace_announced(7, "lobby/bob");

        // Act
        repository.forget_announced_namespaces(7, "room");

        // Assert
        assert!(repository.record_namespace_announced(7, "room/alice"));
        assert!(!repository.record_namespace_announced(7, "lobby/bob"));
    }
}
