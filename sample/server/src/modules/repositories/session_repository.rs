use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use crate::modules::{
    core::{
        publisher::Publisher,
        session::Session,
        subscriber::Subscriber,
    },
    enums::SessionEvent,
    event_resolver::moqt_session_event_resolver::MOQTSessionEventResolver,
    repositories::subscriber_repository::SubscriberRepository,
    thread_manager::ThreadManager,
    types::SessionId,
};

pub(crate) struct SessionRepository {
    thread_manager: ThreadManager,
    sessions: tokio::sync::Mutex<HashMap<SessionId, Arc<dyn Session>>>,
    publishers: tokio::sync::Mutex<HashMap<SessionId, Arc<dyn Publisher>>>,
    subscriber_repo: SubscriberRepository,
}

impl SessionRepository {
    pub(crate) fn new() -> Self {
        Self {
            thread_manager: ThreadManager::new(),
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            publishers: tokio::sync::Mutex::new(HashMap::new()),
            subscriber_repo: SubscriberRepository::new(),
        }
    }

    pub(crate) async fn add(
        &mut self,
        session_id: SessionId,
        session: Box<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) {
        let arc_session: Arc<dyn Session> = Arc::from(session);
        let (publisher, subscriber) = arc_session.new_publisher_subscriber_pair();
        let arc_publisher = Arc::from(publisher);
        self.start_receive(session_id, Arc::downgrade(&arc_session), event_sender);
        self.sessions.lock().await.insert(session_id, arc_session);
        self.publishers
            .lock()
            .await
            .insert(session_id, arc_publisher);
        self.subscriber_repo.add(session_id, subscriber).await;
    }

    fn start_receive(
        &mut self,
        session_id: SessionId,
        session: Weak<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
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
                        tracing::error!("Session has been dropped.");
                        break;
                    }
                }
            })
            .unwrap();
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) async fn get_session(&self, session_id: SessionId) -> Option<Arc<dyn Session>> {
        let sessions = self.sessions.lock().await;
        sessions.get(&session_id).cloned()
    }

    pub(crate) async fn get_subscriber(
        &self,
        session_id: SessionId,
    ) -> Option<Arc<dyn Subscriber>> {
        self.subscriber_repo.get(session_id).await
    }

    pub(crate) async fn get_publisher(&self, session_id: SessionId) -> Option<Arc<dyn Publisher>> {
        let publishers = self.publishers.lock().await;
        let result = publishers.get(&session_id);
        if let Some(publisher) = result {
            Some(publisher.clone())
        } else {
            None
        }
    }
}
