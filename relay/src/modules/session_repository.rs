use std::sync::{Arc, Weak};

use dashmap::DashMap;
use tracing::{Instrument, Span};

use crate::modules::{
    auth::{session_expiry_task::SessionExpiryTask, verified_token::VerifiedToken},
    core::{
        publisher::Publisher, session::Session, session_event::MoqtSessionEvent,
        subscriber::Subscriber,
    },
    session_event::{EventKind, SessionEvent},
    session_event_forward_task_registry::SessionEventForwardTaskRegistry,
    types::SessionId,
};

pub(crate) struct SessionRepository {
    session_event_forward_task_registry: SessionEventForwardTaskRegistry,
    sessions: DashMap<SessionId, Arc<dyn Session>>,
    session_spans: DashMap<SessionId, Span>,
    session_peers: DashMap<SessionId, SessionPeer>,
    session_tokens: DashMap<SessionId, Arc<VerifiedToken>>,
    session_expiry_tasks: DashMap<SessionId, SessionExpiryTask>,
}

pub(crate) struct NewSession {
    pub(crate) session_id: SessionId,
    pub(crate) session: Box<dyn Session>,
    pub(crate) session_span: Span,
    pub(crate) peer: SessionPeer,
    pub(crate) verified_token: VerifiedToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionPeer {
    Client,
    Relay { relay_id: Option<String> },
}

impl SessionPeer {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Relay { .. } => "relay",
        }
    }

    pub(crate) fn relay_id(&self) -> Option<&str> {
        match self {
            Self::Client => None,
            Self::Relay { relay_id } => relay_id.as_deref(),
        }
    }
}

impl SessionRepository {
    pub(crate) fn new() -> Self {
        Self {
            session_event_forward_task_registry: SessionEventForwardTaskRegistry::new(),
            sessions: DashMap::new(),
            session_spans: DashMap::new(),
            session_peers: DashMap::new(),
            session_tokens: DashMap::new(),
            session_expiry_tasks: DashMap::new(),
        }
    }

    pub(crate) async fn add(
        &mut self,
        new_session: NewSession,
        relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) {
        let NewSession {
            session_id,
            session,
            session_span,
            peer,
            verified_token,
        } = new_session;
        let arc_session: Arc<dyn Session> = Arc::from(session);
        tracing::info!(
            session_id = %session_id,
            peer = ?peer,
            app_id = %verified_token.app_id,
            is_relay = verified_token.is_relay,
            "session peer classified"
        );
        self.sessions.insert(session_id, arc_session.clone());
        self.session_spans.insert(session_id, session_span.clone());
        self.session_peers.insert(session_id, peer);
        self.store_verified_token(session_id, &arc_session, verified_token);
        if relay_session_event_sender
            .send(SessionEvent::session_registered(session_id))
            .is_err()
        {
            tracing::error!(
                session_id,
                "relay event channel closed; session is not handled"
            );
        }
        self.start_session_event_forwarding(
            session_id,
            Arc::downgrade(&arc_session),
            relay_session_event_sender,
            session_span,
        );
    }

    pub(crate) fn remove(&mut self, session_id: SessionId) {
        let session_removed = self.sessions.remove(&session_id).is_some();
        let session_span_removed = self.session_spans.remove(&session_id).is_some();
        let session_peer_removed = self.session_peers.remove(&session_id).is_some();
        self.session_tokens.remove(&session_id);
        self.session_expiry_tasks.remove(&session_id);
        self.session_event_forward_task_registry.remove(&session_id);
        tracing::info!(
            session_id = %session_id,
            session_removed,
            session_span_removed,
            session_peer_removed,
            remaining_session_spans = self.session_spans.len(),
            "session removed from repository"
        );
    }

    pub(crate) fn session_span(&self, session_id: SessionId) -> Option<Span> {
        self.session_spans.get(&session_id).map(|span| span.clone())
    }

    pub(crate) fn has_session(&self, session_id: SessionId) -> bool {
        self.sessions.contains_key(&session_id)
    }

    pub(crate) fn peer(&self, session_id: SessionId) -> Option<SessionPeer> {
        self.session_peers
            .get(&session_id)
            .map(|peer| peer.value().clone())
    }

    pub(crate) fn verified_token(&self, session_id: SessionId) -> Option<Arc<VerifiedToken>> {
        self.session_tokens
            .get(&session_id)
            .map(|token| token.value().clone())
    }

    pub(crate) fn replace_verified_token(
        &mut self,
        session_id: SessionId,
        verified_token: VerifiedToken,
    ) -> Option<Arc<VerifiedToken>> {
        let session = self.sessions.get(&session_id)?.value().clone();
        Some(self.store_verified_token(session_id, &session, verified_token))
    }

    fn store_verified_token(
        &self,
        session_id: SessionId,
        session: &Arc<dyn Session>,
        verified_token: VerifiedToken,
    ) -> Arc<VerifiedToken> {
        self.session_expiry_tasks.remove(&session_id);
        if let (false, Some(expires_at)) = (verified_token.is_relay, verified_token.expires_at) {
            self.session_expiry_tasks.insert(
                session_id,
                SessionExpiryTask::run(Arc::downgrade(session), expires_at),
            );
        }
        let verified_token = Arc::new(verified_token);
        self.session_tokens
            .insert(session_id, verified_token.clone());
        verified_token
    }

    pub(crate) fn is_client_session(&self, session_id: SessionId) -> bool {
        matches!(self.peer(session_id), Some(SessionPeer::Client))
    }

    fn start_session_event_forwarding(
        &mut self,
        session_id: SessionId,
        session: Weak<dyn Session>,
        relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        session_span: Span,
    ) {
        let session_event_forwarder_span = tracing::info_span!(
            parent: &session_span,
            "relay.session.event_forwarder",
            session_id = session_id
        );
        let join_handle = tokio::task::Builder::new()
            .name("Session Event Forwarder")
            .spawn(
                async move {
                    loop {
                        if let Some(session) = session.upgrade() {
                            let event = match session.receive_moqt_session_event().await {
                                Ok(event) => event,
                                Err(e) => {
                                    tracing::error!("Failed to receive moqt session event: {}", e);
                                    break;
                                }
                            };
                            let should_stop = matches!(
                                event,
                                MoqtSessionEvent::Disconnected()
                                    | MoqtSessionEvent::ProtocolViolation()
                            );

                            let relay_event = SessionEvent {
                                session_id,
                                kind: EventKind::FromSession(event),
                            };
                            if let Err(err) = relay_session_event_sender.send(relay_event) {
                                tracing::error!("Failed to forward session event: {}", err);
                                break;
                            }
                            if should_stop {
                                tracing::info!("Stopping session event forwarder");
                                break;
                            }
                        } else {
                            tracing::warn!("Session handle no longer available");
                            break;
                        }
                    }
                }
                .instrument(session_event_forwarder_span),
            )
            .unwrap();
        self.session_event_forward_task_registry
            .add(session_id, join_handle);
    }

    pub(crate) fn subscriber(&self, session_id: SessionId) -> Option<Box<dyn Subscriber>> {
        if let Some(session) = self.sessions.get(&session_id) {
            Some(session.value().as_subscriber())
        } else {
            None
        }
    }

    pub(crate) fn close_with_protocol_violation(&self, session_id: SessionId, reason: &str) {
        match self.sessions.get(&session_id) {
            Some(session) => session
                .value()
                .close(moqt::TerminationErrorCode::ProtocolViolation, reason),
            None => tracing::debug!(session_id, "session already gone; nothing to close"),
        }
    }

    pub(crate) fn publisher(&self, session_id: SessionId) -> Option<Box<dyn Publisher>> {
        if let Some(session) = self.sessions.get(&session_id) {
            Some(session.value().as_publisher())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use moqt::TerminationErrorCode;

    use crate::modules::{
        auth::verified_token::VerifiedToken,
        core::mocks::{
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
}
