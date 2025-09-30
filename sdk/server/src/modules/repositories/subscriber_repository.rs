use std::sync::Arc;

use crate::modules::{
    enums::PublisherEvent, subscriber::Subscriber, thread_manager::ThreadManager,
};

pub(crate) struct SubscriberRepository<T: moqt::TransportProtocol> {
    pub(crate) subscribers: tokio::sync::Mutex<Vec<Arc<Subscriber<T>>>>,
    pub(crate) message_sender: tokio::sync::mpsc::UnboundedSender<PublisherEvent>,
    pub(crate) thread_manager: ThreadManager,
}

impl<T: moqt::TransportProtocol> SubscriberRepository<T> {
    pub(crate) async fn add(&mut self, subscriber: Subscriber<T>) {
        let shared_sub = Arc::new(subscriber);
        let weak_sub = Arc::downgrade(&shared_sub);
        self.subscribers.lock().await.push(shared_sub);
        let sender = self.message_sender.clone();
        let join_handle = tokio::task::Builder::new()
            .name("Publisher ID")
            .spawn(async move {
                loop {
                    if let Some(shared_sub) = weak_sub.upgrade() {
                        let event = match shared_sub.receive_from_publisher().await {
                            Ok(event) => event,
                            Err(e) => {
                                tracing::error!("Failed to receive event: {}", e);
                                break;
                            }
                        };
                        sender.send(event);
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
