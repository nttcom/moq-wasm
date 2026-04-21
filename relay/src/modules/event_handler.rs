use std::sync::Arc;
use tokio::sync::mpsc;

use crate::modules::{
    enums::MoqtRelayEvent,
    relay::{egress::coordinator::EgressCommand, ingress::ingress_coordinator::IngressStartRequest},
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
        relay_event_receiver: tokio::sync::mpsc::UnboundedReceiver<MoqtRelayEvent>,
        session_receiver: tokio::sync::mpsc::UnboundedReceiver<MOQTMessageReceived>,
        ingress_sender: mpsc::Sender<IngressStartRequest>,
        egress_sender: mpsc::Sender<EgressCommand>,
    ) -> Self {
        let session_event_watcher = Self::create_pub_sub_event_watcher(
            repo,
            session_receiver,
            ingress_sender,
            egress_sender,
        );
        Self {
            session_event_watcher,
        }
    }

    fn create_pub_sub_event_watcher(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<MoqtRelayEvent>,
        ingress_sender: mpsc::Sender<IngressStartRequest>,
        egress_sender: mpsc::Sender<EgressCommand>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
                let notifier = Notifier { repository: repo };
                let table = Box::new(HashMapTable::new());
                loop {
                    if let Some(event) = receiver.recv().await {
                        match event {
                            MoqtRelayEvent::PublishNameSpace(session_id, handler) => {
                                let publish_ns = PublishNamespace {};
                                publish_ns
                                    .handle(session_id, table.as_ref(), &notifier, handler.as_ref())
                                    .await;
                            }
                            MoqtRelayEvent::SubscribeNameSpace(session_id, handler) => {
                                let subscribe_ns = SubscribeNameSpace {};
                                subscribe_ns
                                    .handle(session_id, table.as_ref(), &notifier, handler.as_ref())
                                    .await;
                            }
                            MoqtRelayEvent::Publish(session_id, handler) => {
                                let publish = Publish {};
                                publish
                                    .handle(session_id, table.as_ref(), &notifier, handler)
                                    .await;
                            }
                            MoqtRelayEvent::Subscribe(session_id, handler) => {
                                let subscribe = Subscribe {};
                                subscribe
                                    .handle(
                                        session_id,
                                        table.as_ref(),
                                        &notifier,
                                        &ingress_sender,
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::Disconnected(session_id) => {
                                tracing::info!("Session disconnected: {}", session_id);
                                table.remove_session(session_id).await;
                                notifier.repository.lock().await.remove(session_id);
                            }
                            MoqtRelayEvent::ProtocolViolation(session_id) => {
                                tracing::error!("Session protocol violation: {}", session_id);
                                table.remove_session(session_id).await;
                                notifier.repository.lock().await.remove(session_id);
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
