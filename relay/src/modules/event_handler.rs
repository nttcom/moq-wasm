use std::sync::Arc;
use tokio::sync::mpsc;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    inter_relay::InterRelayConnectionManager,
    relay::{
        cache::store::TrackCacheStore, egress::coordinator::EgressCommand,
        ingress::ingress_coordinator::IngressCommand,
    },
    route_registry::RelayRouteRegistry,
    sequences::{
        CascadingRelayContext,
        fetch::Fetch,
        publish::Publish,
        publish_namespace::PublishNamespace,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace,
        tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{LocalPubSubDirectory, RemovedSessionSubscriptions},
        },
        unsubscribe::Unsubscribe,
    },
    session_event::SessionEvent,
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};
use tracing::{Instrument, Span};

pub(crate) struct EventHandler {
    relay_session_event_handler: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
        ingress_sender: mpsc::Sender<IngressCommand>,
        egress_sender: mpsc::Sender<EgressCommand>,
        route_registry: Arc<dyn RelayRouteRegistry>,
        inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
        upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
        cache_store: Arc<TrackCacheStore>,
    ) -> Self {
        let relay_session_event_handler = Self::create_relay_session_event_handler(
            repo,
            relay_event_receiver,
            ingress_sender,
            egress_sender,
            route_registry,
            inter_relay_connection_manager,
            upstream_publisher_resolver,
            cache_store,
        );
        Self {
            relay_session_event_handler,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_relay_session_event_handler(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
        ingress_sender: mpsc::Sender<IngressCommand>,
        egress_sender: mpsc::Sender<EgressCommand>,
        route_registry: Arc<dyn RelayRouteRegistry>,
        inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
        upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
        cache_store: Arc<TrackCacheStore>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Relay Session Event Handler")
            .spawn(async move {
                let control_message_forwarder = ControlMessageForwarder {
                    repository: repo.clone(),
                };
                let local_pub_sub_directory = Box::new(InMemoryLocalPubSubDirectory::new());
                loop {
                    if let Some(event) = receiver.recv().await {
                        let session_id = match &event {
                            SessionEvent::PublishNameSpace(session_id, _)
                            | SessionEvent::SubscribeNameSpace(session_id, _)
                            | SessionEvent::Publish(session_id, _)
                            | SessionEvent::Subscribe(session_id, _)
                            | SessionEvent::Unsubscribe(session_id, _)
                            | SessionEvent::Fetch(session_id, _)
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
                        let event_span = Self::session_event_span(&session_span, &event);

                        match event {
                            SessionEvent::PublishNameSpace(session_id, handler) => {
                                let publish_ns = PublishNamespace {};
                                publish_ns
                                    .handle(
                                        session_id,
                                        &session_span,
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        CascadingRelayContext {
                                            route_registry: route_registry.as_ref(),
                                            inter_relay_connection_manager:
                                                inter_relay_connection_manager.as_ref(),
                                        },
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
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        route_registry.as_ref(),
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
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        CascadingRelayContext {
                                            route_registry: route_registry.as_ref(),
                                            inter_relay_connection_manager:
                                                inter_relay_connection_manager.as_ref(),
                                        },
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
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        &ingress_sender,
                                        &egress_sender,
                                        upstream_publisher_resolver.as_ref(),
                                        &cache_store,
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
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        &ingress_sender,
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            SessionEvent::Fetch(session_id, handler) => {
                                let fetch = Fetch {};
                                fetch
                                    .handle(
                                        session_id,
                                        &session_span,
                                        local_pub_sub_directory.as_ref(),
                                        &egress_sender,
                                        handler,
                                    )
                                    .await;
                            }
                            SessionEvent::Disconnected(session_id) => {
                                let disconnected_span = tracing::info_span!(
                                    parent: &event_span,
                                    "relay.session.disconnected",
                                    session_id = session_id
                                );
                                async {
                                    tracing::info!("Session disconnected: {}", session_id);
                                    let removed =
                                        local_pub_sub_directory.remove_session(session_id).await;
                                    Self::cleanup_removed_session(
                                        session_id,
                                        removed,
                                        &control_message_forwarder,
                                        &ingress_sender,
                                        &egress_sender,
                                    )
                                    .await;
                                    control_message_forwarder
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
                                    parent: &event_span,
                                    "relay.session.protocol_violation",
                                    session_id = session_id
                                );
                                async {
                                    tracing::error!("Session protocol violation: {}", session_id);
                                    let removed =
                                        local_pub_sub_directory.remove_session(session_id).await;
                                    Self::cleanup_removed_session(
                                        session_id,
                                        removed,
                                        &control_message_forwarder,
                                        &ingress_sender,
                                        &egress_sender,
                                    )
                                    .await;
                                    control_message_forwarder
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

    fn session_event_span(session_span: &Span, event: &SessionEvent) -> Span {
        match event {
            SessionEvent::PublishNameSpace(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishNamespace",
                track_namespace = %handler.track_namespace(),
            ),
            SessionEvent::SubscribeNameSpace(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "SubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
            ),
            SessionEvent::Publish(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Publish",
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                track_alias = handler.track_alias(),
            ),
            SessionEvent::Subscribe(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Subscribe",
                subscribe_id = handler.subscribe_id(),
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
            ),
            SessionEvent::Unsubscribe(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Unsubscribe",
                subscribe_id = handler.subscribe_id(),
            ),
            SessionEvent::Disconnected(session_id) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Disconnected",
            ),
            SessionEvent::Fetch(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Fetch",
                request_id = handler.request_id(),
            ),
            SessionEvent::ProtocolViolation(session_id) => tracing::info_span!(
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
        control_message_forwarder: &ControlMessageForwarder,
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
                    && let Err(err) = control_message_forwarder
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
        self.relay_session_event_handler.abort();
    }
}
