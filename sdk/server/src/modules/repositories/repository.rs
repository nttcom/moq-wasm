use std::{os::unix::thread, sync::Arc};

use crate::modules::{
    enums::MOQTEvent, repositories::repository_trait::RepositoryTrait,
    thread_manager::ThreadManager,
};

struct Repository {
    publisher_repository: RepositoryTrait,
    subscriber_repository: RepositoryTrait,
    message_sender: tokio::sync::mpsc::UnboundedSender<MOQTEvent>,
    thread_manager: ThreadManager,
}

impl Repository {
    pub(crate) fn add(&self, publisher: moqt::Publisher) {
        let shared_pub = Arc::new(publisher);
        let sender = self.message_sender.clone();
        let join_handle = tokio::task::Builder::new()
            .name("Publisher ID")
            .spawn(async move {
                loop {
                    let _ = shared_pub.receive_from_subscriber().await;
                    sender.send(message)
                }
            })
            .unwrap();
        self.thread_manager.add_join_handle(join_handle);
    }

    pub(crate) fn remove() {}

    pub(crate) fn send() {}

    pub(crate) fn receive() {}
}
