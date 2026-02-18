use std::sync::{Arc, Weak};

use dashmap::DashMap;

use crate::modules::{
    core::{publisher::Publisher, session::Session, subscriber::Subscriber},
    enums::MOQTMessageReceived,
    event_resolver::moqt_session_event_resolver::MOQTSessionEventResolver,
    thread_manager::ThreadManager,
    types::SessionId,
};

pub(crate) struct SessionRepository {
    thread_manager: ThreadManager,
    sessions: DashMap<SessionId, Arc<dyn Session>>,
}

impl SessionRepository {
    pub(crate) fn new() -> Self {
        Self {
            thread_manager: ThreadManager::new(),
            sessions: DashMap::new(),
        }
    }

    pub(crate) async fn add(
        &mut self,
        session_id: SessionId,
        session: Box<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<MOQTMessageReceived>,
    ) {
        let arc_session: Arc<dyn Session> = Arc::from(session);
        self.start_receive(session_id, Arc::downgrade(&arc_session), event_sender);
        self.sessions.insert(session_id, arc_session);
    }

    fn start_receive(
        &mut self,
        session_id: SessionId,
        session: Weak<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<MOQTMessageReceived>,
    ) {
        let join_handle = tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                loop {
                    if let Some(session) = session.upgrade() {
                        let event = session.receive_session_event().await;
                        if let Err(e) = event {
                            tracing::error!("Failed to receive session event: {}", e);
                            break;
                        }
                        let session_event =
                            MOQTSessionEventResolver::resolve(session_id, event.unwrap());
                        event_sender.send(session_event).unwrap();
                    } else {
                        tracing::error!("Session dropped.");
                        break;
                    }
                }
            })
            .unwrap();
        self.thread_manager.add(session_id, join_handle);
    }

    pub(crate) async fn subscriber(&self, session_id: SessionId) -> Option<Box<dyn Subscriber>> {
        if let Some(session) = self.sessions.get(&session_id) {
            Some(session.value().as_subscriber())
        } else {
            None
        }
    }

    pub(crate) async fn publisher(&self, session_id: SessionId) -> Option<Box<dyn Publisher>> {
        if let Some(session) = self.sessions.get(&session_id) {
            Some(session.value().as_publisher())
        } else {
            None
        }
    }
}
