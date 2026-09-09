use std::sync::{Arc, Weak};

use dashmap::DashMap;
use tracing::{Instrument, Span};

use crate::modules::{
    auth::verified_token::VerifiedToken,
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

fn filter_type_label(filter_type: &crate::modules::enums::FilterType) -> String {
    match filter_type {
        crate::modules::enums::FilterType::NextGroupStart => "NextGroupStart".to_string(),
        crate::modules::enums::FilterType::LargestObject => "LargestObject".to_string(),
        crate::modules::enums::FilterType::AbsoluteStart { location } => format!(
            "AbsoluteStart(group_id={}, object_id={})",
            location.group_id, location.object_id
        ),
        crate::modules::enums::FilterType::AbsoluteRange {
            location,
            end_group,
        } => format!(
            "AbsoluteRange(group_id={}, object_id={}, end_group={})",
            location.group_id, location.object_id, end_group
        ),
    }
}

fn log_session_event(event: &MoqtSessionEvent) {
    match event {
        MoqtSessionEvent::PublishNamespace(handler) => {
            tracing::info!(
                event = "PublishNamespace",
                track_namespace = %handler.track_namespace(),
                "Received session event"
            );
        }
        MoqtSessionEvent::PublishNamespaceDone(handler) => {
            tracing::info!(
                event = "PublishNamespaceDone",
                track_namespace = %handler.track_namespace(),
                "Received session event"
            );
        }
        MoqtSessionEvent::SubscribeNamespace(handler) => {
            tracing::info!(
                event = "SubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
                "Received session event"
            );
        }
        MoqtSessionEvent::UnsubscribeNamespace(handler) => {
            tracing::info!(
                event = "UnsubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
                "Received session event"
            );
        }
        MoqtSessionEvent::Publish(handler) => {
            tracing::info!(
                event = "Publish",
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                track_alias = handler.track_alias(),
                group_order = ?handler._group_order(),
                content_exists = ?handler._content_exists(),
                forward = handler._forward(),
                delivery_timeout = ?handler._delivery_timeout(),
                max_cache_duration = ?handler._max_cache_duration(),
                "Received session event"
            );
        }
        MoqtSessionEvent::Subscribe(handler) => {
            let filter_type = handler._filter_type();
            tracing::info!(
                event = "Subscribe",
                subscribe_id = handler.subscribe_id(),
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                subscriber_priority = handler._subscriber_priority(),
                group_order = ?handler._group_order(),
                forward = handler._forward(),
                filter_type = %filter_type_label(&filter_type),
                max_cache_duration = ?handler._max_cache_duration(),
                delivery_timeout = ?handler._delivery_timeout(),
                "Received session event"
            );
        }
        MoqtSessionEvent::Unsubscribe(handler) => {
            tracing::info!(
                event = "Unsubscribe",
                subscribe_id = handler.subscribe_id(),
                "Received session event"
            );
        }
        MoqtSessionEvent::Fetch(handler) => {
            tracing::info!(
                event = "Fetch",
                request_id = handler.request_id(),
                fetch_params = ?handler.fetch_params(),
                "Received session event"
            );
        }
        MoqtSessionEvent::FetchCancel(handler) => {
            tracing::info!(
                event = "FetchCancel",
                request_id = handler.request_id(),
                "Received session event"
            );
        }
        MoqtSessionEvent::GoAway(handler) => {
            tracing::info!(
                event = "GoAway",
                new_session_uri = %handler.new_session_uri(),
                "Received session event"
            );
        }
        MoqtSessionEvent::MaxRequestId(handler) => {
            tracing::info!(
                event = "MaxRequestId",
                request_id = handler.request_id(),
                "Received session event"
            );
        }
        MoqtSessionEvent::RequestsBlocked(handler) => {
            tracing::info!(
                event = "RequestsBlocked",
                maximum_request_id = handler.maximum_request_id(),
                "Received session event"
            );
        }
        MoqtSessionEvent::PublishDone(handler) => {
            tracing::info!(
                event = "PublishDone",
                request_id = handler.request_id(),
                status_code = handler.status_code(),
                stream_count = handler.stream_count(),
                error_reason = %handler.error_reason(),
                "Received session event"
            );
        }
        MoqtSessionEvent::PublishNamespaceCancel(handler) => {
            tracing::info!(
                event = "PublishNamespaceCancel",
                track_namespace = %handler.track_namespace(),
                error_code = handler.error_code(),
                error_reason = %handler.error_reason(),
                "Received session event"
            );
        }
        MoqtSessionEvent::SubscribeUpdate(handler) => {
            tracing::info!(
                event = "SubscribeUpdate",
                request_id = handler.request_id(),
                subscription_request_id = handler.subscription_request_id(),
                start_location = ?handler.start_location(),
                end_group = handler.end_group(),
                subscriber_priority = handler.subscriber_priority(),
                forward = handler.forward(),
                "Received session event"
            );
        }
        MoqtSessionEvent::TrackStatus(handler) => {
            tracing::info!(
                event = "TrackStatus",
                request_id = handler.request_id(),
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                "Received session event"
            );
        }
        MoqtSessionEvent::Disconnected() => {
            tracing::info!(event = "Disconnected", "Received session event");
        }
        MoqtSessionEvent::ProtocolViolation() => {
            tracing::error!(event = "ProtocolViolation", "Received session event");
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
        self.session_tokens
            .insert(session_id, Arc::new(verified_token));
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

    #[allow(dead_code)]
    pub(crate) fn verified_token(&self, session_id: SessionId) -> Option<Arc<VerifiedToken>> {
        self.session_tokens
            .get(&session_id)
            .map(|token| token.value().clone())
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
                                Ok(event) => {
                                    log_session_event(&event);
                                    event
                                }
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
            Some(session) => session.value().close_with_protocol_violation(reason),
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
    use crate::modules::{
        auth::verified_token::VerifiedToken, core::mocks::session_repository_with_upstream_session,
    };

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
            *recorded.protocol_violation_reasons.lock().unwrap(),
            vec!["invalid object status 0x2".to_string()]
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
        assert!(
            recorded
                .protocol_violation_reasons
                .lock()
                .unwrap()
                .is_empty()
        );
    }
}
