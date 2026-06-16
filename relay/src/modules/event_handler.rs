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
        publish_namespace_done::PublishNamespaceDone,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace,
        tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{
                LocalPubSubDirectory, RemovedSessionSubscriptions, UpstreamSubscriptionOrigin,
            },
        },
        unsubscribe::Unsubscribe,
        unsubscribe_namespace::UnsubscribeNamespace,
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
                let local_pub_sub_directory: Arc<dyn LocalPubSubDirectory> =
                    Arc::new(InMemoryLocalPubSubDirectory::new());
                let mut request_response_tasks: tokio::task::JoinSet<()> =
                    tokio::task::JoinSet::new();
                loop {
                    let event = tokio::select! {
                        event = receiver.recv() => {
                            let Some(event) = event else {
                                tracing::error!("Failed to receive session event");
                                break;
                            };
                            event
                        }
                        result = request_response_tasks.join_next(), if !request_response_tasks.is_empty() => {
                            match result {
                                Some(Ok(())) => {
                                    tracing::debug!("request/response task completed");
                                }
                                Some(Err(error)) => {
                                    tracing::error!(?error, "request/response task failed");
                                }
                                None => {}
                            }
                            continue;
                        }
                    };

                    let session_id = match &event {
                        SessionEvent::PublishNameSpace(session_id, _)
                        | SessionEvent::PublishNamespaceDone(session_id, _)
                        | SessionEvent::SubscribeNameSpace(session_id, _)
                        | SessionEvent::UnsubscribeNameSpace(session_id, _)
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
                            // Awaits PUBLISH_NAMESPACE_OK/ERROR from an upstream peer —
                            // must be spawned to avoid blocking the event loop.
                            let control_message_forwarder = control_message_forwarder.clone();
                            let local_pub_sub_directory = local_pub_sub_directory.clone();
                            let route_registry = route_registry.clone();
                            let inter_relay_connection_manager =
                                inter_relay_connection_manager.clone();
                            request_response_tasks.spawn(async move {
                                PublishNamespace {}
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
                            });
                        }
                        SessionEvent::PublishNamespaceDone(session_id, handler) => {
                            // Fire-and-forget: no peer response to wait for — stays inline.
                            let publish_ns_done = PublishNamespaceDone {};
                            publish_ns_done
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
                            // Awaits SUBSCRIBE_NAMESPACE_OK/ERROR from an upstream peer —
                            // must be spawned to avoid blocking the event loop.
                            let control_message_forwarder = control_message_forwarder.clone();
                            let local_pub_sub_directory = local_pub_sub_directory.clone();
                            let route_registry = route_registry.clone();
                            request_response_tasks.spawn(async move {
                                SubscribeNameSpace {}
                                    .handle(
                                        session_id,
                                        &session_span,
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        route_registry.as_ref(),
                                        handler.as_ref(),
                                    )
                                    .await;
                            });
                        }
                        SessionEvent::UnsubscribeNameSpace(session_id, handler) => {
                            // No peer response expected — stays inline.
                            let unsubscribe_ns = UnsubscribeNamespace {};
                            unsubscribe_ns
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
                        SessionEvent::Publish(session_id, handler) => {
                            // Awaits PUBLISH_OK/ERROR from a downstream peer —
                            // must be spawned to avoid blocking the event loop.
                            let control_message_forwarder = control_message_forwarder.clone();
                            let local_pub_sub_directory = local_pub_sub_directory.clone();
                            let ingress_sender = ingress_sender.clone();
                            let route_registry = route_registry.clone();
                            let inter_relay_connection_manager =
                                inter_relay_connection_manager.clone();
                            request_response_tasks.spawn(async move {
                                Publish {}
                                    .handle(
                                        session_id,
                                        &session_span,
                                        local_pub_sub_directory.as_ref(),
                                        &control_message_forwarder,
                                        &ingress_sender,
                                        CascadingRelayContext {
                                            route_registry: route_registry.as_ref(),
                                            inter_relay_connection_manager:
                                                inter_relay_connection_manager.as_ref(),
                                        },
                                        handler,
                                    )
                                    .await;
                            });
                        }
                        SessionEvent::Subscribe(session_id, handler) => {
                            // Awaits SUBSCRIBE_OK/ERROR from an upstream peer —
                            // must be spawned to avoid blocking the event loop.
                            // TODO(deadlock-core): serialize upstream creation per track to avoid duplicate upstream subscribe
                            let control_message_forwarder = control_message_forwarder.clone();
                            let local_pub_sub_directory = local_pub_sub_directory.clone();
                            let ingress_sender = ingress_sender.clone();
                            let egress_sender = egress_sender.clone();
                            let upstream_publisher_resolver = upstream_publisher_resolver.clone();
                            let cache_store = cache_store.clone();
                            request_response_tasks.spawn(async move {
                                Subscribe {}
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
                            });
                        }
                        SessionEvent::Unsubscribe(session_id, handler) => {
                            // No upstream peer response — stays inline.
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
                            // No upstream peer response (only local table + egress) — stays inline.
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
                                    local_pub_sub_directory.as_ref(),
                                    &control_message_forwarder,
                                    &ingress_sender,
                                    &egress_sender,
                                    route_registry.as_ref(),
                                    inter_relay_connection_manager.as_ref(),
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
                                    local_pub_sub_directory.as_ref(),
                                    &control_message_forwarder,
                                    &ingress_sender,
                                    &egress_sender,
                                    route_registry.as_ref(),
                                    inter_relay_connection_manager.as_ref(),
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
            SessionEvent::PublishNamespaceDone(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishNamespaceDone",
                track_namespace = %handler.track_namespace(),
            ),
            SessionEvent::SubscribeNameSpace(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "SubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
            ),
            SessionEvent::UnsubscribeNameSpace(session_id, handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "UnsubscribeNamespace",
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

    #[allow(clippy::too_many_arguments)]
    async fn cleanup_removed_session(
        removed_session_id: SessionId,
        removed: RemovedSessionSubscriptions,
        table: &dyn LocalPubSubDirectory,
        control_message_forwarder: &ControlMessageForwarder,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        egress_sender: &mpsc::Sender<EgressCommand>,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
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

            if removed_downstream.remaining_downstream_subscriber_count == 0
                && removed_downstream.upstream_origin == UpstreamSubscriptionOrigin::Subscribe
            {
                if removed_downstream.upstream_key.publisher_session_id != removed_session_id
                    && let Err(err) = control_message_forwarder
                        .unsubscribe(
                            removed_downstream.upstream_key.publisher_session_id,
                            removed_downstream.upstream_request_id,
                        )
                        .await
                {
                    tracing::debug!(
                        ?err,
                        upstream_session_id = removed_downstream.upstream_key.publisher_session_id,
                        request_id = removed_downstream.upstream_request_id,
                        "failed to forward upstream unsubscribe during session cleanup"
                    );
                }

                Self::stop_ingress_track(ingress_sender, removed_downstream.track_key).await;
            }
        }

        for track_key in removed.upstream_track_keys {
            Self::stop_ingress_track(ingress_sender, track_key).await;
        }

        if control_message_forwarder
            .repository
            .lock()
            .await
            .is_client_session(removed_session_id)
        {
            for track_namespace_prefix in removed.subscribe_namespace_prefixes {
                UnsubscribeNamespace::cleanup_empty_namespace_subscription(
                    &track_namespace_prefix,
                    table,
                    control_message_forwarder,
                    route_registry,
                    inter_relay_connection_manager,
                )
                .await;
            }

            for track_namespace in removed.publish_namespace_track_namespaces {
                PublishNamespaceDone::notify_local_subscribers(
                    removed_session_id,
                    &track_namespace,
                    table,
                    control_message_forwarder,
                )
                .await;
                PublishNamespaceDone::withdraw_namespace_publication(
                    &track_namespace,
                    control_message_forwarder,
                    route_registry,
                    inter_relay_connection_manager,
                )
                .await;
            }
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
