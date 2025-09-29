use std::sync::Arc;

use moqt::Subscriber;

use crate::modules::{enums::MOQTEvent, thread_manager::ThreadManager};

pub(crate) struct SubscriberRepository {
    subscribers: tokio::sync::Mutex<Vec<Arc<Subscriber>>>,
    message_sender: tokio::sync::mpsc::UnboundedSender<MOQTEvent>,
    thread_manager: ThreadManager,
}

impl SubscriberRepository {
    pub(crate) async fn add(&mut self, subscriber: Subscriber) {
        let shared_sub = Arc::new(subscriber);
        let weak_sub = Arc::downgrade(&shared_sub);
        self.subscribers.lock().await.push(shared_sub);
        let sender = self.message_sender.clone();
        let join_handle = tokio::task::Builder::new()
            .name("Publisher ID")
            .spawn(async move {
                loop {
                    if let Some(shared_sub) = weak_sub.upgrade() {
                        let _ = shared_sub.receive_from_publisher().await;
                        sender.send(message);
                    } else {
                        tracing::error!("Publisher has been deleted.");
                        break;
                    }
                }
            })
            .unwrap();
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn remove() {}

    pub(crate) fn send() {}

    pub(crate) fn receive() {}
}
