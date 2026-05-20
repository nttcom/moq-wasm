use std::sync::Arc;
use tokio::sync::mpsc;

use crate::modules::{
    enums::MoqtRelayEvent,
    relay::{egress::coordinator::EgressCommand, ingress::ingress_coordinator::IngressCommand},
    sequences::{
        notifier::Notifier,
        publish::Publish,
        publish_namespace::PublishNamespace,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace,
        tables::{
            hashmap_table::HashMapTable,
            table::{RemovedSessionSubscriptions, Table},
        },
        unsubscribe::Unsubscribe,
    },
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
};
use tracing::{Instrument, Span};

pub(crate) struct EventHandler {
    session_event_watcher: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub(crate) fn run(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_event_receiver: tokio::sync::mpsc::UnboundedReceiver<MoqtRelayEvent>,
        ingress_sender: mpsc::Sender<IngressCommand>,
        egress_sender: mpsc::Sender<EgressCommand>,
    ) -> Self {
        let session_event_watcher = Self::create_pub_sub_event_watcher(
            repo,
            relay_event_receiver,
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
        ingress_sender: mpsc::Sender<IngressCommand>,
        egress_sender: mpsc::Sender<EgressCommand>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Session Event Watcher")
            .spawn(async move {
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
                            | MoqtRelayEvent::Unsubscribe(session_id, _)
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
                        let event_span = Self::session_event_span(&session_span, &event);

                        match event {
                            MoqtRelayEvent::PublishNameSpace(session_id, handler) => {
                                let publish_ns = PublishNamespace {};
                                publish_ns
                                    .handle(
                                        session_id,
                                        &event_span,
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
                                        &event_span,
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
                                        &event_span,
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
                                        &event_span,
                                        table.as_ref(),
                                        &notifier,
                                        &ingress_sender,
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::Unsubscribe(session_id, handler) => {
                                let unsubscribe = Unsubscribe {};
                                unsubscribe
                                    .handle(
                                        session_id,
                                        &event_span,
                                        table.as_ref(),
                                        &notifier,
                                        &ingress_sender,
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            MoqtRelayEvent::Disconnected(session_id) => {
                                let disconnected_span = tracing::info_span!(
                                    parent: &event_span,
                                    "relay.session.disconnected",
                                    session_id = session_id
                                );
                                async {
                                    tracing::info!("Session disconnected: {}", session_id);
                                    let removed = table.remove_session(session_id).await;
                                    Self::cleanup_removed_session(
                                        session_id,
                                        removed,
                                        &notifier,
                                        &ingress_sender,
                                        &egress_sender,
                                    )
                                    .await;
                                    notifier.repository.lock().await.remove(session_id);
                                }
                                .instrument(disconnected_span)
                                .await;
                            }
                            MoqtRelayEvent::ProtocolViolation(session_id) => {
                                let protocol_violation_span = tracing::info_span!(
                                    parent: &event_span,
                                    "relay.session.protocol_violation",
                                    session_id = session_id
                                );
                                async {
                                    tracing::error!("Session protocol violation: {}", session_id);
                                    let removed = table.remove_session(session_id).await;
                                    Self::cleanup_removed_session(
                                        session_id,
                                        removed,
                                        &notifier,
                                        &ingress_sender,
                                        &egress_sender,
                                    )
                                    .await;
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

    fn session_event_span(session_span: &Span, event: &MoqtRelayEvent) -> Span {
        match event {
            MoqtRelayEvent::PublishNameSpace(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishNamespace",
                track_namespace = %handler.track_namespace(),
            ),
            MoqtRelayEvent::SubscribeNameSpace(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "SubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
            ),
            MoqtRelayEvent::Publish(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Publish",
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                track_alias = handler.track_alias(),
            ),
            MoqtRelayEvent::Subscribe(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Subscribe",
                subscribe_id = handler.subscribe_id(),
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
            ),
            MoqtRelayEvent::Unsubscribe(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Unsubscribe",
                subscribe_id = handler.subscribe_id(),
            ),
            MoqtRelayEvent::Disconnected(session_id) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Disconnected",
            ),
            MoqtRelayEvent::ProtocolViolation(session_id) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "ProtocolViolation",
            ),
        }
    }

    async fn cleanup_removed_session(
        removed_session_id: SessionId,
        removed: RemovedSessionSubscriptions,
        notifier: &Notifier,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        egress_sender: &mpsc::Sender<EgressCommand>,
    ) {
        for removed_downstream in removed.downstream_subscriptions {
            if egress_sender
                .send(EgressCommand::StopReader {
                    subscriber_session_id: removed_downstream.downstream_session_id,
                    downstream_subscribe_id: removed_downstream.downstream_subscribe_id,
                })
                .await
                .is_err()
            {
                tracing::debug!(
                    session_id = removed_downstream.downstream_session_id,
                    subscribe_id = removed_downstream.downstream_subscribe_id,
                    "failed to send egress stop request"
                );
            }

            if removed_downstream.remaining_downstream_subscriber_count == 0 {
                if removed_downstream.upstream_key.publisher_session_id != removed_session_id
                    && let Err(err) = notifier
                        .unsubscribe(
                            removed_downstream.upstream_key.publisher_session_id,
                            removed_downstream.upstream_subscribe_id,
                        )
                        .await
                {
                    tracing::debug!(
                        ?err,
                        upstream_session_id = removed_downstream.upstream_key.publisher_session_id,
                        subscribe_id = removed_downstream.upstream_subscribe_id,
                        "failed to forward upstream unsubscribe during session cleanup"
                    );
                }

                Self::stop_ingress_track(ingress_sender, removed_downstream.track_key).await;
            }
        }

        for track_key in removed.upstream_track_keys {
            Self::stop_ingress_track(ingress_sender, track_key).await;
        }
    }

    async fn stop_ingress_track(
        ingress_sender: &mpsc::Sender<IngressCommand>,
        track_key: TrackKey,
    ) {
        if ingress_sender
            .send(IngressCommand::StopTrack { track_key })
            .await
            .is_err()
        {
            tracing::debug!(track_key, "failed to send ingress stop request");
        }
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        tracing::info!("Manager dropped.");
        self.session_event_watcher.abort();
    }
}
