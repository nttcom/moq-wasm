use std::sync::Arc;

use crate::modules::{
    enums::MoqtRelayEvent,
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
use tracing::Instrument;

pub(crate) struct EventHandler {
    session_event_watcher: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub(crate) fn run(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_event_receiver: tokio::sync::mpsc::UnboundedReceiver<MoqtRelayEvent>,
    ) -> Self {
        let session_event_watcher = Self::create_pub_sub_event_watcher(repo, relay_event_receiver);
        Self {
            session_event_watcher,
        }
    }

    fn create_pub_sub_event_watcher(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<MoqtRelayEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                let mut stream_handler = StreamBinder::new(repo.clone());
                let notifier = Notifier {
                    repository: repo.clone(),
                };
                let table = Box::new(HashMapTable::new());
                loop {
                    if let Some(event) = receiver.recv().await {
                        let session_id = match &event {
                            MoqtRelayEvent::PublishNameSpace(session_id, _)
                            | MoqtRelayEvent::SubscribeNameSpace(session_id, _)
                            | MoqtRelayEvent::Publish(session_id, _)
                            | MoqtRelayEvent::Subscribe(session_id, _)
                            | MoqtRelayEvent::Disconnected(session_id)
                            | MoqtRelayEvent::ProtocolViolation(session_id) => *session_id,
                        };

                        let session_span = {
                            let repo = repo.lock().await;
                            repo.session_span(session_id)
                        };
                        let Some(session_span) = session_span else {
                            tracing::warn!("Session span not found: {}", session_id);
                            continue;
                        };

                        match event {
                            MoqtRelayEvent::PublishNameSpace(session_id, handler) => {
                                let publish_ns = PublishNamespace {};
                                publish_ns
                                    .handle(
                                        session_id,
                                        &session_span,
                                        table.as_ref(),
                                        &notifier,
                                        handler.as_ref(),
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::SubscribeNameSpace(session_id, handler) => {
                                let subscribe_ns = SubscribeNameSpace {};
                                subscribe_ns
                                    .handle(
                                        session_id,
                                        &session_span,
                                        table.as_ref(),
                                        &notifier,
                                        handler.as_ref(),
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::Publish(session_id, handler) => {
                                let publish = Publish {};
                                publish
                                    .handle(
                                        session_id,
                                        &session_span,
                                        table.as_ref(),
                                        &notifier,
                                        handler,
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::Subscribe(session_id, handler) => {
                                let subscribe = Subscribe {};
                                subscribe
                                    .handle(
                                        session_id,
                                        &session_span,
                                        table.as_ref(),
                                        &notifier,
                                        &mut stream_handler,
                                        handler,
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::Disconnected(session_id) => {
                                let disconnected_span = tracing::info_span!(
                                    parent: session_span,
                                    "relay.session.disconnected",
                                    session_id = session_id
                                );
                                async {
                                    tracing::info!("Session disconnected: {}", session_id);
                                    table.remove_session(session_id).await;
                                    notifier.repository.lock().await.remove(session_id);
                                }
                                .instrument(disconnected_span)
                                .await;
                            }
                            MoqtRelayEvent::ProtocolViolation(session_id) => {
                                let protocol_violation_span = tracing::info_span!(
                                    parent: session_span,
                                    "relay.session.protocol_violation",
                                    session_id = session_id
                                );
                                async {
                                    tracing::error!("Session protocol violation: {}", session_id);
                                    table.remove_session(session_id).await;
                                    notifier.repository.lock().await.remove(session_id);
                                }
                                .instrument(protocol_violation_span)
                                .await;
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
        tracing::info!("Manager dropped.");
        self.session_event_watcher.abort();
    }
}
