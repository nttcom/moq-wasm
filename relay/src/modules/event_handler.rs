use std::sync::Arc;

use crate::modules::{
    enums::MOQTMessageReceived,
    event_resolver::stream_binder::StreamBinder,
    sequences::{
        notifier::Notifier,
        publish::Publish,
        publish_namespace::PublishNamespace,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace,
        tables::{hashmap_table::HashMapTable, table::Table},
    },
    session_repository::SessionRepository,
};

pub(crate) struct EventHandler {
    session_event_watcher: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub(crate) fn run(
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
                let mut stream_handler = StreamBinder::new(repo.clone());
                let notifier = Notifier { repository: repo };
                let table = Box::new(HashMapTable::new());
                loop {
                    if let Some(event) = receiver.recv().await {
                        match event {
                            MOQTMessageReceived::PublishNameSpace(session_id, handler) => {
                                let publish_ns = PublishNamespace {};
                                publish_ns
                                    .handle(session_id, table.as_ref(), &notifier, handler.as_ref())
                                    .await;
                            }
                            MOQTMessageReceived::SubscribeNameSpace(session_id, handler) => {
                                let subscribe_ns = SubscribeNameSpace {};
                                subscribe_ns
                                    .handle(session_id, table.as_ref(), &notifier, handler.as_ref())
                                    .await;
                            }
                            MOQTMessageReceived::Publish(session_id, handler) => {
                                let publish = Publish {};
                                publish
                                    .handle(session_id, table.as_ref(), &notifier, handler)
                                    .await;
                            }
                            MOQTMessageReceived::Subscribe(session_id, handler) => {
                                let subscribe = Subscribe {};
                                subscribe
                                    .handle(
                                        session_id,
                                        table.as_ref(),
                                        &notifier,
                                        &mut stream_handler,
                                        handler,
                                    )
                                    .await;
                            }
                            MOQTMessageReceived::Disconnected(session_id) => {
                                tracing::info!("Session disconnected: {}", session_id);
                                table.remove_session(session_id).await;
                                notifier.repository.lock().await.remove(session_id);
                            }
                            MOQTMessageReceived::ProtocolViolation() => todo!(),
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
        tracing::info!("Manager dropped.");
        self.session_event_watcher.abort();
    }
}
