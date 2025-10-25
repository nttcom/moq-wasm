use std::sync::Arc;

use crate::modules::{
    enums::MOQTMessageReceived, event_resolver::sequence_handler::SequenceHandler,
    repositories::session_repository::SessionRepository,
};

pub(crate) struct EventHandler {
    session_event_watcher: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn run(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_receiver: tokio::sync::mpsc::UnboundedReceiver<MOQTMessageReceived>,
    ) -> Self {
        let session_event_watcher = Self::create_pub_sub_event_watcher(repo, session_receiver);
        Self {
            session_event_watcher,
        }
    }

    fn create_pub_sub_event_watcher(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<MOQTMessageReceived>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                let mut sequense_handler = SequenceHandler::new(repo);
                loop {
                    if let Some(event) = receiver.recv().await {
                        match event {
                            MOQTMessageReceived::PublishNameSpace(session_id, items) => {
                                sequense_handler.publish_namespace(session_id, items).await
                            }
                            MOQTMessageReceived::SubscribeNameSpace(session_id, items) => {
                                sequense_handler
                                    .subscribe_namespace(session_id, items)
                                    .await;
                            }
                            MOQTMessageReceived::Publish(
                                session_id,
                                namespaces,
                                track_name,
                                track_alias,
                                group_order,
                                is_content_exist,
                                location,
                                is_forward,
                                delivery_timeout,
                                max_cache_duration,
                            ) => {
                                sequense_handler
                                    .publish(
                                        session_id,
                                        namespaces,
                                        track_name,
                                        track_alias,
                                        group_order,
                                        is_content_exist,
                                        location,
                                        is_forward,
                                        delivery_timeout,
                                        max_cache_duration,
                                    )
                                    .await
                            }
                            MOQTMessageReceived::Subscribe(
                                session_id,
                                namespaces,
                                track_name,
                                track_alias,
                                subscriber_priority,
                                group_order,
                                is_content_exist,
                                is_forward,
                                filter_type,
                                delivery_timeout,
                            ) => {
                                sequense_handler
                                    .subscribe(
                                        session_id,
                                        namespaces,
                                        track_name,
                                        track_alias,
                                        subscriber_priority,
                                        group_order,
                                        is_content_exist,
                                        is_forward,
                                        filter_type,
                                        delivery_timeout,
                                    )
                                    .await
                            }
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

impl Drop for EventHandler {
    fn drop(&mut self) {
        tracing::info!("Manager has been dropped.");
        self.session_event_watcher.abort();
    }
}
