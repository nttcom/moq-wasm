use std::sync::Arc;

use crate::modules::{
    enums::SessionEvent, event_resolver::sequence_handler::SequenceHandler,
    repositories::session_repository::SessionRepository,
};

pub(crate) struct Manager {
    session_event_watcher: tokio::task::JoinHandle<()>,
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl Manager {
    pub fn run(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    ) -> Self {
        let session_event_watcher =
            Self::create_pub_sub_event_watcher(repo.clone(), session_receiver);
        Self {
            repo,
            session_event_watcher,
        }
    }

    fn create_pub_sub_event_watcher(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                let mut sequense_handler = SequenceHandler::new(repo);
                loop {
                    if let Some(event) = receiver.recv().await {
                        match event {
                            SessionEvent::PublishNameSpace(session_id, items) => {
                                sequense_handler.publish_namespace(session_id, items).await
                            }
                            SessionEvent::SubscribeNameSpace(session_id, items) => {
                                sequense_handler
                                    .subscribe_namespace(session_id, items)
                                    .await;
                            }
                            SessionEvent::Publish(
                                session_id,
                                _,
                                items,
                                _,
                                _,
                                _,
                                _,
                                _,
                                items1,
                                items2,
                                items3,
                            ) => todo!(),
                            SessionEvent::Subscribe(
                                session_id,
                                _,
                                items,
                                _,
                                _,
                                _,
                                _,
                                _,
                                items1,
                                items2,
                            ) => todo!(),
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

impl Drop for Manager {
    fn drop(&mut self) {
        tracing::info!("Manager has been dropped.");
        self.session_event_watcher.abort();
    }
}
