use std::sync::{Arc, Weak};

use dashmap::DashMap;
use tracing::{Instrument, Span};

use crate::modules::{
    core::{
        publisher::Publisher, session::Session, session_event::SessionEvent, subscriber::Subscriber,
    },
    enums::MoqtRelayEvent,
    event_resolver::moqt_relay_event_resolver::MoqtRelayEventResolver,
    thread_manager::ThreadManager,
    types::SessionId,
};

pub(crate) struct SessionRepository {
    thread_manager: ThreadManager,
    sessions: DashMap<SessionId, Arc<dyn Session>>,
    session_spans: DashMap<SessionId, Span>,
}

impl SessionRepository {
    pub(crate) fn new() -> Self {
        Self {
            thread_manager: ThreadManager::new(),
            sessions: DashMap::new(),
            session_spans: DashMap::new(),
        }
    }

    pub(crate) async fn add(
        &mut self,
        session_id: SessionId,
        session: Box<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<MoqtRelayEvent>,
        session_span: Span,
    ) {
        let arc_session: Arc<dyn Session> = Arc::from(session);
        self.sessions.insert(session_id, arc_session.clone());
        self.session_spans.insert(session_id, session_span.clone());
        self.start_receive(
            session_id,
            Arc::downgrade(&arc_session),
            event_sender,
            session_span,
        );
    }

    pub(crate) fn remove(&mut self, session_id: SessionId) {
        self.sessions.remove(&session_id);
        self.session_spans.remove(&session_id);
        self.thread_manager.remove(&session_id);
    }

    pub(crate) fn session_span(&self, session_id: SessionId) -> Option<Span> {
        self.session_spans.get(&session_id).map(|span| span.clone())
    }

    fn start_receive(
        &mut self,
        session_id: SessionId,
        session: Weak<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<MoqtRelayEvent>,
        session_span: Span,
    ) {
        let session_event_watcher_span = tracing::info_span!(
            parent: &session_span,
            "relay.session.event_watcher",
            session_id = session_id
        );
        let join_handle = tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(
                async move {
                    loop {
                        if let Some(session) = session.upgrade() {
                            let event = match session.receive_session_event().await {
                                Ok(event) => {
                                    tracing::info!(
                                        event = %format!("{event:?}"),
                                        "Received session event"
                                    );
                                    event
                                }
                                Err(e) => {
                                    tracing::error!("Failed to receive session event: {}", e);
                                    break;
                                }
                            };
                            let should_stop = matches!(
                                event,
                                SessionEvent::Disconnected() | SessionEvent::ProtocolViolation()
                            );

                            let relay_event = MoqtRelayEventResolver::resolve(session_id, event);
                            if let Err(err) = event_sender.send(relay_event) {
                                tracing::error!("Failed to forward session event: {}", err);
                                break;
                            }
                            if should_stop {
                                tracing::info!("Stopping session event watcher");
                                break;
                            }
                        } else {
                            tracing::warn!("Session handle no longer available");
                            break;
                        }
                    }
                }
                .instrument(session_event_watcher_span),
            )
            .unwrap();
        self.thread_manager.add(session_id, join_handle);
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
