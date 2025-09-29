use std::sync::Arc;

use crate::modules::{
    enums::MOQTEvent, publisher::Publisher,
    thread_manager::ThreadManager,
};

pub(crate) struct PublisherRepository {
    publishers: tokio::sync::Mutex<Vec<Arc<Publisher>>>,
    message_sender: tokio::sync::mpsc::UnboundedSender<MOQTEvent>,
    thread_manager: ThreadManager,
}

impl PublisherRepository {
    pub(crate) async fn add(&mut self, publisher: Publisher) {
        let shared_pub = Arc::new(publisher);
        let weak_pub = Arc::downgrade(&shared_pub);
        self.publishers.lock().await.push(shared_pub);
        let sender = self.message_sender.clone();
        let join_handle = tokio::task::Builder::new()
            .name("Publisher ID")
            .spawn(async move {
                loop {
                    if let Some(shared_pub) = weak_pub.upgrade() {
                        let _ = shared_pub.receive_from_subscriber().await;
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
