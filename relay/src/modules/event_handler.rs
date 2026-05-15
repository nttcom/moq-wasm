use std::sync::Arc;
use tokio::sync::mpsc;

use crate::modules::{
    relay::{
        egress::coordinator::EgressCommand, ingress::ingress_coordinator::IngressStartRequest,
    },
    sequences::{
        notifier::SessionSignalingDispatcher,
        publish::Publish,
        publish_namespace::PublishNamespace,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace,
        tables::{hashmap_table::InMemorySignalingStateTable, table::SignalingStateTable},
        unsubscribe::Unsubscribe,
    },
    session_event::SessionEvent,
    session_repository::SessionRepository,
};
use tracing::Instrument;

pub(crate) struct EventHandler {
    relay_session_event_handler: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub(crate) fn run(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
        ingress_sender: mpsc::Sender<IngressStartRequest>,
        egress_sender: mpsc::Sender<EgressCommand>,
    ) -> Self {
        let relay_session_event_handler = Self::create_relay_session_event_handler(
            repo,
            relay_event_receiver,
            ingress_sender,
            egress_sender,
        );
        Self {
            relay_session_event_handler,
        }
    }

    fn create_relay_session_event_handler(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
        ingress_sender: mpsc::Sender<IngressStartRequest>,
        egress_sender: mpsc::Sender<EgressCommand>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Relay Session Event Handler")
            .spawn(async move {
                let session_signaling_dispatcher = SessionSignalingDispatcher {
                    repository: repo.clone(),
                };
                let signaling_state_table = Box::new(InMemorySignalingStateTable::new());
                loop {
                    if let Some(event) = receiver.recv().await {
                        let session_id = match &event {
                            SessionEvent::PublishNameSpace(session_id, _)
                            | SessionEvent::SubscribeNameSpace(session_id, _)
                            | SessionEvent::Publish(session_id, _)
                            | SessionEvent::Subscribe(session_id, _)
                            | SessionEvent::Unsubscribe(session_id, _)
                            | SessionEvent::Disconnected(session_id)
                            | SessionEvent::ProtocolViolation(session_id) => *session_id,
                        };

                        let session_span = {
                            let repo: tokio::sync::MutexGuard<'_, SessionRepository> =
                                repo.lock().await;
                            repo.session_span(session_id)
                        };
                        let Some(session_span) = session_span else {
                            tracing::warn!("Session span not found: {}", session_id);
                            continue;
                        };

                        match event {
                            SessionEvent::PublishNameSpace(session_id, handler) => {
                                let publish_ns = PublishNamespace {};
                                publish_ns
                                    .handle(
                                        session_id,
                                        &session_span,
                                        signaling_state_table.as_ref(),
                                        &session_signaling_dispatcher,
                                        handler.as_ref(),
                                    )
                                    .await;
                            }
                            SessionEvent::SubscribeNameSpace(session_id, handler) => {
                                let subscribe_ns = SubscribeNameSpace {};
                                subscribe_ns
                                    .handle(
                                        session_id,
                                        &session_span,
                                        signaling_state_table.as_ref(),
                                        &session_signaling_dispatcher,
                                        handler.as_ref(),
                                    )
                                    .await;
                            }
                            SessionEvent::Publish(session_id, handler) => {
                                let publish = Publish {};
                                publish
                                    .handle(
                                        session_id,
                                        &session_span,
                                        signaling_state_table.as_ref(),
                                        &session_signaling_dispatcher,
                                        handler,
                                    )
                                    .await;
                            }
                            SessionEvent::Subscribe(session_id, handler) => {
                                let subscribe = Subscribe {};
                                subscribe
                                    .handle(
                                        session_id,
                                        &session_span,
                                        signaling_state_table.as_ref(),
                                        &session_signaling_dispatcher,
                                        &ingress_sender,
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            SessionEvent::Unsubscribe(session_id, handler) => {
                                let unsubscribe = Unsubscribe {};
                                unsubscribe
                                    .handle(
                                        session_id,
                                        &session_span,
                                        signaling_state_table.as_ref(),
                                        &session_signaling_dispatcher,
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            SessionEvent::Disconnected(session_id) => {
                                let disconnected_span = tracing::info_span!(
                                    parent: session_span,
                                    "relay.session.disconnected",
                                    session_id = session_id
                                );
                                async {
                                    tracing::info!("Session disconnected: {}", session_id);
                                    signaling_state_table.remove_session(session_id).await;
                                    session_signaling_dispatcher
                                        .repository
                                        .lock()
                                        .await
                                        .remove(session_id);
                                }
                                .instrument(disconnected_span)
                                .await;
                            }
                            SessionEvent::ProtocolViolation(session_id) => {
                                let protocol_violation_span = tracing::info_span!(
                                    parent: session_span,
                                    "relay.session.protocol_violation",
                                    session_id = session_id
                                );
                                async {
                                    tracing::error!("Session protocol violation: {}", session_id);
                                    signaling_state_table.remove_session(session_id).await;
                                    session_signaling_dispatcher
                                        .repository
                                        .lock()
                                        .await
                                        .remove(session_id);
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
        self.relay_session_event_handler.abort();
    }
}
