use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use crate::modules::{
    core::subscriber::Subscriber, thread_manager::ThreadManager, types::SessionId,
};

pub(crate) struct SubscriberRepository {
    thread_manager: ThreadManager,
    subscribers: tokio::sync::Mutex<HashMap<SessionId, Arc<dyn Subscriber>>>,
}

impl SubscriberRepository {
    pub(crate) fn new() -> Self {
        Self {
            thread_manager: ThreadManager::new(),
            subscribers: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn add(&mut self, session_id: SessionId, subscriber: Box<dyn Subscriber>) {
        let arc_subscriber = Arc::from(subscriber);
        // self.start_receive(uuid, Arc::downgrade(&arc_session), event_sender);
        self.subscribers
            .lock()
            .await
            .insert(session_id, arc_subscriber);
    }

    pub(crate) async fn get(&self, session_id: SessionId) -> Option<Arc<dyn Subscriber>> {
        let subscribers = self.subscribers.lock().await;
        let result = subscribers.get(&session_id);
        if let Some(publisher) = result {
            Some(publisher.clone())
        } else {
            None
        }
    }
}
