use std::sync::{Arc, Weak};

use dashmap::DashMap;
use tracing::{Instrument, Span};

use crate::modules::{
    core::{
        publisher::Publisher, session::Session, session_event::MoqtSessionEvent,
        subscriber::Subscriber,
    },
    event_resolver::moqt_relay_event_resolver::RelaySessionEventResolver,
    session_event::SessionEvent,
    session_event_forward_task_registry::SessionEventForwardTaskRegistry,
    types::SessionId,
};

pub(crate) struct SessionRepository {
    session_event_forward_task_registry: SessionEventForwardTaskRegistry,
    sessions: DashMap<SessionId, Arc<dyn Session>>,
    session_spans: DashMap<SessionId, Span>,
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
        MoqtSessionEvent::SubscribeNamespace(handler) => {
            tracing::info!(
                event = "SubscribeNamespace",
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
                has_authorization_token = handler._authorization_token().is_some(),
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
                has_authorization_token = handler._authorization_token().is_some(),
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
        }
    }

    pub(crate) async fn add(
        &mut self,
        session_id: SessionId,
        session: Box<dyn Session>,
        relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        session_span: Span,
    ) {
        let arc_session: Arc<dyn Session> = Arc::from(session);
        self.sessions.insert(session_id, arc_session.clone());
        self.session_spans.insert(session_id, session_span.clone());
        self.start_session_event_forwarding(
            session_id,
            Arc::downgrade(&arc_session),
            relay_session_event_sender,
            session_span,
        );
    }

    pub(crate) fn remove(&mut self, session_id: SessionId) {
        self.sessions.remove(&session_id);
        self.session_spans.remove(&session_id);
        self.session_event_forward_task_registry.remove(&session_id);
    }

    pub(crate) fn session_span(&self, session_id: SessionId) -> Option<Span> {
        self.session_spans.get(&session_id).map(|span| span.clone())
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

                            let relay_event = RelaySessionEventResolver::resolve(session_id, event);
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

    pub(crate) fn publisher(&self, session_id: SessionId) -> Option<Box<dyn Publisher>> {
        if let Some(session) = self.sessions.get(&session_id) {
            Some(session.value().as_publisher())
        } else {
            None
        }
    }
}
