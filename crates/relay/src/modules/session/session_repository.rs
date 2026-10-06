use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use tracing::{Instrument, Span};

use crate::modules::{
    auth::{session_expiry_task::SessionExpiryTask, verified_token::VerifiedToken},
    domain::{session_id::SessionId, session_peer::SessionPeer},
    session::{
        Session,
        moqt_session_event::MoqtSessionEvent,
        publisher::Publisher,
        session_event::{EventKind, SessionEvent},
        subscriber::Subscriber,
    },
};

pub(crate) struct SessionRepository {
    sessions: HashMap<SessionId, SessionEntry>,
}

struct SessionEntry {
    session: Arc<dyn Session>,
    span: Span,
    peer: SessionPeer,
    verified_token: Arc<VerifiedToken>,
    expiry_task: Option<SessionExpiryTask>,
    _event_forward_task: SessionEventForwardTask,
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
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
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
        let session: Arc<dyn Session> = Arc::from(session);
        tracing::info!(
            session_id = %session_id,
            peer = ?peer,
            app_id = %verified_token.app_id,
            is_relay = verified_token.is_relay,
            "session peer classified"
        );
        let expiry_task = expiry_task(&session, &verified_token);
        if relay_session_event_sender
            .send(SessionEvent::session_registered(session_id))
            .is_err()
        {
            tracing::error!(
                session_id,
                "relay event channel closed; session is not handled"
            );
        }
        let event_forward_task = SessionEventForwardTask::run(
            session_id,
            Arc::downgrade(&session),
            relay_session_event_sender,
            &session_span,
        );
        self.sessions.insert(
            session_id,
            SessionEntry {
                session,
                span: session_span,
                peer,
                verified_token: Arc::new(verified_token),
                expiry_task,
                _event_forward_task: event_forward_task,
            },
        );
    }

    pub(crate) fn remove(&mut self, session_id: SessionId) {
        let session_removed = self.sessions.remove(&session_id).is_some();
        tracing::info!(
            session_id = %session_id,
            session_removed,
            remaining_sessions = self.sessions.len(),
            "session removed from repository"
        );
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

    pub(crate) fn close_with_protocol_violation(&self, session_id: SessionId, reason: &str) {
        match self.sessions.get(&session_id) {
            Some(entry) => entry
                .session
                .close(moqt::TerminationErrorCode::ProtocolViolation, reason),
            None => tracing::debug!(session_id, "session already gone; nothing to close"),
        }
    }
}

struct SessionEventForwardTask {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionEventForwardTask {
    fn run(
        session_id: SessionId,
        session: Weak<dyn Session>,
        relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        session_span: &Span,
    ) -> Self {
        let session_event_forwarder_span = tracing::info_span!(
            parent: session_span,
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
        Self { join_handle }
    }
}

impl Drop for SessionEventForwardTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use moqt::TerminationErrorCode;

    use crate::modules::{
        auth::verified_token::VerifiedToken,
        session::mocks::{
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
