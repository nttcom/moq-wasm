use std::collections::HashMap;

use crate::modules::{
    enums::MOQTEvent,
    message_handler::MessageHandler,
    namespace_table::NamespaceTable,
    publisher::Publisher,
    repositories::{
        publisher_repository::PublisherRepository, subscriber_repository::SubscriberRepository,
    },
    subscriber::Subscriber,
    thread_manager::ThreadManager,
};

pub(crate) struct Manager {
    join_handle: tokio::task::JoinHandle<()>,
}

impl Manager {
    pub fn run<T: moqt::TransportProtocol>(
        receiver: tokio::sync::mpsc::Receiver<(Publisher<T>, Subscriber<T>)>,
    ) -> Self {
        let pub_repo = PublisherRepository::<T> {
            publishers: tokio::sync::Mutex::new(vec![]),
            message_sender: todo!(),
            thread_manager: ThreadManager::new(),
        };
        let sub_repo = SubscriberRepository::<T> {
            subscribers: tokio::sync::Mutex::new(vec![]),
            message_sender: todo!(),
            thread_manager: ThreadManager::new(),
        };
        let join_handle = Self::create_session_event_watcher(receiver, pub_repo, sub_repo);
        Self { join_handle }
    }

    fn create_session_event_watcher<T: moqt::TransportProtocol>(
        mut receiver: tokio::sync::mpsc::Receiver<(Publisher<T>, Subscriber<T>)>,
        mut pub_repo: PublisherRepository<T>,
        mut sub_repo: SubscriberRepository<T>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                loop {
                    if let Some((publisher, subscriber)) = receiver.recv().await {
                        pub_repo.add(publisher).await;
                        sub_repo.add(subscriber).await;
                    } else {
                        tracing::error!("Failed to receive session event");
                        break;
                    }
                }
            })
            .unwrap()
    }

    fn create_pub_sub_event_watcher(
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<MOQTEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                let table = tokio::sync::Mutex::new(HashMap::new());
                let mut pub_table = NamespaceTable { table };
                let table = tokio::sync::Mutex::new(HashMap::new());
                let mut sub_table = NamespaceTable { table };
                loop {
                    if let Some(event) = receiver.recv().await {
                        match event {
                            MOQTEvent::NamespacePublished(sub_id, ns) => {
                                MessageHandler::publish_namespace(&mut sub_table, sub_id, ns)
                            }
                            MOQTEvent::NamespaceSubscribed(pub_id, ns) => {
                                MessageHandler::subscribe_namespace(&mut pub_table, pub_id, ns)
                            }
                            MOQTEvent::Publish() => todo!(),
                            MOQTEvent::Subscribe() => todo!(),
                        }
                    } else {
                        tracing::error!("Failed to receive session event");
                        break;
                    }
                }
            })
            .unwrap()
    }
}
