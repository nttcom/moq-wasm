use std::sync::Arc;

use uuid::Uuid;

use crate::modules::{
    core::{publisher::Publisher, session::Session, subscriber::Subscriber},
    enums::SessionEvent,
    event_resolver::sequence_handler::SequenceHandler,
    repositories::session_repository::SessionRepository,
};

pub(crate) struct Manager {
    new_session_watcher: tokio::task::JoinHandle<()>,
    session_event_watcher: tokio::task::JoinHandle<()>,
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl Manager {
    pub fn run(
        receiver: tokio::sync::mpsc::UnboundedReceiver<(
            Uuid,
            Box<dyn Session>,
            Box<dyn Publisher>,
            Box<dyn Subscriber>,
        )>,
    ) -> Self {
        let repo: Arc<tokio::sync::Mutex<SessionRepository>> =
            Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let (session_sender, session_receiver) =
            tokio::sync::mpsc::unbounded_channel::<SessionEvent>();

        let new_session_watcher =
            Self::create_new_session_watcher(receiver, session_sender, repo.clone());
        let session_event_watcher =
            Self::create_pub_sub_event_watcher(repo.clone(), session_receiver);
        Self {
            new_session_watcher,
            repo,
            session_event_watcher,
        }
    }

    fn create_new_session_watcher(
        mut event_receiver: tokio::sync::mpsc::UnboundedReceiver<(
            Uuid,
            Box<dyn Session>,
            Box<dyn Publisher>,
            Box<dyn Subscriber>,
        )>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                loop {
                    if let Some((uuid, session, publisher, subscriber)) =
                        event_receiver.recv().await
                    {
                        tracing::info!("Session event received");
                        repo.lock()
                            .await
                            .add(
                                uuid,
                                session,
                                session_event_sender.clone(),
                                publisher,
                                subscriber,
                            )
                            .await;
                    } else {
                        tracing::error!("Failed to receive session event");
                        break;
                    }
                }
            })
            .unwrap()
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
                            SessionEvent::PublishNameSpace(uuid, items) => {
                                sequense_handler.publish_namespace(uuid, items)
                            }
                            SessionEvent::SubscribeNameSpace(uuid, items) => {
                                sequense_handler.subscribe_namespace(uuid, items);
                            }
                            SessionEvent::Publish(
                                uuid,
                                _,
                                items,
                                _,
                                _,
                                _,
                                items1,
                                items2,
                                items3,
                            ) => todo!(),
                            SessionEvent::Subscribe(
                                uuid,
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
