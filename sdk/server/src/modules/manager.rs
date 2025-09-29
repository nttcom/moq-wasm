use std::collections::HashMap;

use crate::modules::{
    enums::MOQTEvent,
    message_handler::MessageHandler,
    namespace_table::NamespaceTable,
    repositories::{
        publisher_repository::PublisherRepository, subscriber_repository::SubscriberRepository,
    },
};

pub(crate) struct Manager {
    join_handle: tokio::task::JoinHandle<()>,
    pub_repo: PublisherRepository,
    sub_repo: SubscriberRepository,
}

impl Manager {
    pub fn run(receiver: tokio::sync::mpsc::Receiver<(moqt::Publisher, moqt::Subscriber)>) {}

    fn create_session_event_watcher(
        mut receiver: tokio::sync::mpsc::Receiver<(moqt::Publisher, moqt::Subscriber)>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                loop {
                    if let Some((publisher, subscriber)) = receiver.recv().await {
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
