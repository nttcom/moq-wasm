use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use uuid::Uuid;

use crate::modules::{
    core::{publisher::Publisher, session::Session, subscriber::Subscriber},
    enums::SessionEvent,
    thread_manager::ThreadManager,
};

pub(crate) struct Repository {
    thread_manager: ThreadManager,
    sessions: tokio::sync::Mutex<HashMap<Uuid, Arc<dyn Session>>>,
    publishers: tokio::sync::Mutex<HashMap<Uuid, Box<dyn Publisher>>>,
    subscribers: tokio::sync::Mutex<HashMap<Uuid, Box<dyn Subscriber>>>,
}

impl Repository {
    pub(crate) fn new() -> Self {
        Self {
            thread_manager: ThreadManager::new(),
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            publishers: tokio::sync::Mutex::new(HashMap::new()),
            subscribers: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn add(
        &self,
        uuid: Uuid,
        session: Box<dyn Session>,
        publisher: Box<dyn Publisher>,
        subscriber: Box<dyn Subscriber>,
    ) {
        let arc_session = Arc::from(session);
        self.start_receive(uuid, Arc::downgrade(&arc_session));
        self.sessions.lock().await.insert(uuid, arc_session);
        self.publishers.lock().await.insert(uuid, publisher);
        self.subscribers.lock().await.insert(uuid, subscriber);
    }

    fn start_receive(
        &self,
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
                        let session_event = Self::resolve_session_event(uuid, event.unwrap());
                        event_sender.send(session_event).unwrap();
                    } else {
                        tracing::error!("Session has been dropped.");
                        break;
                    }
                }
            })
            .unwrap();
    }

    fn resolve_session_event(uuid: Uuid, event: moqt::SessionEvent) -> SessionEvent {
        match event {
            moqt::SessionEvent::PublishNameSpace(request_id, namespaces, param) => {
                SessionEvent::PublishNameSpace(uuid, request_id, namespaces, param)
            }
            moqt::SessionEvent::SubscribeNameSpace(request_id, namespaces, param) => {
                SessionEvent::PublishNameSpace(uuid, request_id, namespaces, param)
            }
            moqt::SessionEvent::Publish(
                request_id,
                namespaces,
                group_order,
                is_content_exist,
                is_forward,
                param,
            ) => todo!(),
            moqt::SessionEvent::Subscribe(
                request_id,
                namespaces,
                subscriber_priority,
                group_order,
                is_content_exist,
                is_forward,
                filter_type,
                param,
            ) => todo!(),
        }
    }
}
