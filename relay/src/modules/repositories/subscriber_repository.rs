use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::{core::subscriber::Subscriber, types::SessionId};

pub(crate) struct SubscriberRepository {
    subscribers: DashMap<SessionId, Arc<dyn Subscriber>>,
}

impl SubscriberRepository {
    pub(crate) fn new() -> Self {
        Self {
            subscribers: DashMap::new(),
        }
    }

    pub(crate) async fn add(&mut self, session_id: SessionId, subscriber: Box<dyn Subscriber>) {
        let arc_subscriber = Arc::from(subscriber);
        // self.start_receive(uuid, Arc::downgrade(&arc_session), event_sender);
        self.subscribers.insert(session_id, arc_subscriber);
    }

    pub(crate) async fn get(&self, session_id: SessionId) -> Option<Arc<dyn Subscriber>> {
        let subscribers = self.subscribers.get(&session_id);
        if let Some(publisher) = subscribers {
            Some(publisher.clone())
        } else {
            None
        }
    }
}
