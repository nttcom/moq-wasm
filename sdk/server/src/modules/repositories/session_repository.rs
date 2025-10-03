use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use uuid::Uuid;

use crate::modules::{
    core::{publisher::Publisher, session::Session, subscriber::Subscriber},
    enums::SessionEvent,
    event_resolver::moqt_session_event_resolver::MOQTSessionEventResolver,
    repositories::subscriber_repository::SubscriberRepository,
    thread_manager::ThreadManager,
};

pub(crate) struct SessionRepository {
    thread_manager: ThreadManager,
    sessions: tokio::sync::Mutex<HashMap<Uuid, Arc<dyn Session>>>,
    publishers: tokio::sync::Mutex<HashMap<Uuid, Arc<dyn Publisher>>>,
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
        uuid: Uuid,
        session: Box<dyn Session>,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        publisher: Box<dyn Publisher>,
        subscriber: Box<dyn Subscriber>,
    ) {
        let arc_session = Arc::from(session);
        let arc_publisher = Arc::from(publisher);
        self.start_receive(uuid, Arc::downgrade(&arc_session), event_sender);
        self.sessions.lock().await.insert(uuid, arc_session);
        self.publishers.lock().await.insert(uuid, arc_publisher);
        self.subscriber_repo.add(uuid, subscriber).await;
    }

    fn start_receive(
        &mut self,
        uuid: Uuid,
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
                        let session_event = MOQTSessionEventResolver::resolve(uuid, event.unwrap());
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

    pub(crate) async fn get_subscriber(&self, uuid: Uuid) -> Option<Arc<dyn Subscriber>> {
        self.subscriber_repo.get(uuid).await
    }

    pub(crate) async fn get_publisher(&self, uuid: Uuid) -> Option<Arc<dyn Publisher>> {
        let publishers = self.publishers.lock().await;
        let result = publishers.get(&uuid);
        if let Some(publisher) = result {
            Some(publisher.clone())
        } else {
            None
        }
    }
}
