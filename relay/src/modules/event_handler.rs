use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::modules::{
    auth::{
        request_gate::{authorize_request, reject_unauthorized},
        token_refresh::refresh_token,
        token_verifier::TokenVerifier,
    },
    control_message_forwarder::ControlMessageForwarder,
    core::session_event::MoqtSessionEvent,
    enums::SubscribeErrorCode,
    inter_relay::InterRelayConnectionManager,
    relay::{
        cache::store::TrackCacheStore, egress::coordinator::EgressCommand,
        ingress::ingress_coordinator::IngressCommand,
    },
    route_registry::RelayRouteRegistry,
    sequences::{
        CascadingRelayContext,
        fetch::Fetch,
        malformed_track::MalformedTrackCleanup,
        publish::Publish,
        publish_namespace::PublishNamespace,
        publish_namespace_done::PublishNamespaceDone,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNameSpace,
        tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{RemovedSessionSubscriptions, UpstreamSubscriptionOrigin},
        },
        unsubscribe::Unsubscribe,
        unsubscribe_namespace::UnsubscribeNamespace,
        upstream_serializer::UpstreamCreationSerializer,
    },
    session_event::{EventKind, SessionEvent},
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};
use tracing::{Instrument, Span};

pub(crate) struct EventHandler {
    relay_session_event_handler: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct WorkerDeps {
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) relay_event_sender: mpsc::UnboundedSender<SessionEvent>,
    pub(crate) control_message_forwarder: ControlMessageForwarder,
    pub(crate) local_pub_sub_directory: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) ingress_sender: mpsc::Sender<IngressCommand>,
    pub(crate) egress_sender: mpsc::Sender<EgressCommand>,
    pub(crate) route_registry: Arc<dyn RelayRouteRegistry>,
    pub(crate) inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    pub(crate) upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) upstream_serializer: UpstreamCreationSerializer,
    pub(crate) token_verifier: Arc<dyn TokenVerifier>,
}

impl EventHandler {
    pub(crate) fn run(
        mut receiver: mpsc::UnboundedReceiver<SessionEvent>,
        deps: WorkerDeps,
    ) -> Self {
        let relay_session_event_handler = tokio::task::Builder::new()
            .name("Relay Session Event Handler")
            .spawn(async move {
                let mut sender_map: HashMap<SessionId, mpsc::UnboundedSender<SessionEvent>> =
                    HashMap::new();

                let mut workers: tokio::task::JoinSet<SessionId> =
                    tokio::task::JoinSet::new();

                loop {
                    tokio::select! {
                        result = workers.join_next(), if !workers.is_empty() => {
                            match result {
                                Some(Ok(session_id)) => {
                                    sender_map.remove(&session_id);
                                    tracing::debug!(session_id, "session worker exited");
                                }
                                Some(Err(error)) => {
                                    tracing::error!(?error, "session worker panicked");
                                }
                                None => {}
                            }
                        }
                        // Pull the next event.  The reader NEVER awaits a peer
                        // response — it only dispatches to per-session channels.
                        // Cross-session deadlock is structurally impossible.
                        event = receiver.recv() => {
                            let Some(event) = event else {
                                tracing::info!("Session event channel closed; shutting down reader");
                                break;
                            };

                            let session_id = event.session_id;

                            if matches!(event.kind, EventKind::SessionRegistered) {
                                let (tx, rx) = mpsc::unbounded_channel::<SessionEvent>();
                                workers.spawn(Self::session_worker(session_id, rx, deps.clone()));
                                sender_map.insert(session_id, tx);
                                continue;
                            }

                            let Some(sender) = sender_map.get(&session_id) else {
                                tracing::debug!(session_id, "session has no worker; event dropped");
                                continue;
                            };
                            // Unbounded send never blocks, so the reader never
                            // stalls on a slow or blocked worker.
                            if sender.send(event).is_err() {
                                tracing::debug!(session_id, "session has no worker; event dropped");
                            }
                        }
                    }
                }

                // Drop sender_map so every worker observes channel close and exits.
                drop(sender_map);
                while workers.join_next().await.is_some() {}
            })
            .unwrap();
        Self {
            relay_session_event_handler,
        }
    }

    /// Handles one session's events strictly in order: each handler, including
    /// its peer round-trips, completes before the next event is pulled.
    async fn session_worker(
        session_id: SessionId,
        mut rx: mpsc::UnboundedReceiver<SessionEvent>,
        deps: WorkerDeps,
    ) -> SessionId {
        let WorkerDeps {
            repo,
            relay_event_sender,
            control_message_forwarder,
            local_pub_sub_directory,
            ingress_sender,
            egress_sender,
            route_registry,
            inter_relay_connection_manager,
            upstream_publisher_resolver,
            cache_store,
            upstream_serializer,
            token_verifier,
        } = deps;
        let cascading_relay_context = || CascadingRelayContext {
            route_registry: route_registry.as_ref(),
            inter_relay_connection_manager: inter_relay_connection_manager.as_ref(),
        };
        let (session_span, mut verified_token) = {
            let repo = repo.lock().await;
            (
                repo.session_span(session_id),
                repo.verified_token(session_id),
            )
        };
        // The span is registered before SessionRegistered is sent and removed
        // only by this worker's terminal cleanup.
        let session_span = session_span
            .unwrap_or_else(|| unreachable!("worker started for a session without a span"));
        if verified_token.is_none() {
            tracing::error!(
                session_id,
                "session has no verified token; every namespace request will be denied"
            );
        }

        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(
                event.kind,
                EventKind::FromSession(
                    MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation()
                )
            );

            let event = match event.kind {
                // The reader consumes registrations to spawn this worker and never forwards them.
                EventKind::SessionRegistered => unreachable!("registration forwarded to a worker"),
                EventKind::FromSession(event) => event,
                EventKind::MalformedTrackDetected(track_key) => {
                    let event_span = tracing::info_span!(
                        parent: &session_span,
                        "relay.session.event",
                        session_id = %session_id,
                        event = "MalformedTrackDetected",
                        track_key = %track_key,
                    );
                    MalformedTrackCleanup {}
                        .handle(
                            session_id,
                            &session_span,
                            &track_key,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                        )
                        .instrument(event_span)
                        .await;
                    continue;
                }
                EventKind::ProtocolViolationDetected { reason } => {
                    tracing::error!(
                        parent: &session_span,
                        session_id,
                        %reason,
                        "protocol violation detected; closing session"
                    );
                    repo.lock()
                        .await
                        .close_with_protocol_violation(session_id, &reason);
                    continue;
                }
            };
            let event_span = Self::session_event_span(session_id, &session_span, &event);
            event_span.in_scope(|| match event {
                MoqtSessionEvent::ProtocolViolation() => tracing::error!("Received session event"),
                _ => tracing::info!("Received session event"),
            });
            if let Err(denied) = authorize_request(verified_token.as_deref(), &event) {
                let reject_span = tracing::info_span!(
                    parent: &session_span,
                    "relay.session.unauthorized_request",
                    session_id = %session_id,
                    event = ?event,
                    reason = denied.reason,
                );
                reject_unauthorized(event, denied)
                    .instrument(reject_span)
                    .await;
                continue;
            }

            match event {
                MoqtSessionEvent::PublishNamespace(handler) => {
                    PublishNamespace {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            cascading_relay_context(),
                            handler.as_ref(),
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::PublishNamespaceDone(handler) => {
                    PublishNamespaceDone {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            cascading_relay_context(),
                            &handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::SubscribeNamespace(handler) => {
                    SubscribeNameSpace {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            route_registry.as_ref(),
                            handler.as_ref(),
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::UnsubscribeNamespace(handler) => {
                    UnsubscribeNamespace {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            cascading_relay_context(),
                            &handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::Publish(handler) => {
                    Publish {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                            cascading_relay_context(),
                            handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::Subscribe(handler) => {
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
                            &upstream_serializer,
                            handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::Unsubscribe(handler) => {
                    Unsubscribe {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                            handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::Fetch(handler) => {
                    Fetch {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &cache_store,
                            &egress_sender,
                            &relay_event_sender,
                            &control_message_forwarder,
                            upstream_publisher_resolver.as_ref(),
                            handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                MoqtSessionEvent::TrackStatus(handler) => {
                    async {
                        let refreshed = refresh_token(
                            token_verifier.as_ref(),
                            verified_token.as_deref(),
                            handler.authorization_tokens(),
                        )
                        .await;
                        let response = match refreshed {
                            Ok(token) => {
                                match repo.lock().await.replace_verified_token(session_id, token) {
                                    Some(token) => {
                                        tracing::info!(
                                            expires_at = ?token.expires_at,
                                            "authorization token refreshed"
                                        );
                                        verified_token = Some(token);
                                        handler.ok().await
                                    }
                                    None => {
                                        handler
                                            .error(
                                                SubscribeErrorCode::InternalError as u64,
                                                "session not found".to_string(),
                                            )
                                            .await
                                    }
                                }
                            }
                            Err(rejected) => {
                                tracing::warn!(
                                    code = ?rejected.code,
                                    reason = %rejected.reason,
                                    "authorization token refresh rejected"
                                );
                                handler.error(rejected.code as u64, rejected.reason).await
                            }
                        };
                        if let Err(error) = response {
                            tracing::warn!(?error, "failed to answer TRACK_STATUS");
                        }
                    }
                    .instrument(event_span)
                    .await;
                }
                MoqtSessionEvent::GoAway(..)
                | MoqtSessionEvent::MaxRequestId(..)
                | MoqtSessionEvent::RequestsBlocked(..)
                | MoqtSessionEvent::PublishNamespaceCancel(..)
                | MoqtSessionEvent::PublishDone(..)
                | MoqtSessionEvent::SubscribeUpdate(..)
                | MoqtSessionEvent::FetchCancel(..) => {
                    event_span.in_scope(|| {
                        tracing::warn!("Relay handling for this event is not implemented");
                    });
                }
                MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation() => {
                    let terminal_span = if matches!(event, MoqtSessionEvent::Disconnected()) {
                        let span = tracing::info_span!(
                            parent: &event_span,
                            "relay.session.disconnected",
                            session_id = session_id
                        );
                        span.in_scope(|| tracing::info!("Session disconnected: {}", session_id));
                        span
                    } else {
                        let span = tracing::info_span!(
                            parent: &event_span,
                            "relay.session.protocol_violation",
                            session_id = session_id
                        );
                        span.in_scope(|| {
                            tracing::error!("Session protocol violation: {}", session_id)
                        });
                        span
                    };
                    Self::cleanup_session(
                        session_id,
                        local_pub_sub_directory.as_ref(),
                        &control_message_forwarder,
                        &ingress_sender,
                        route_registry.as_ref(),
                        inter_relay_connection_manager.as_ref(),
                    )
                    .instrument(terminal_span)
                    .await;
                }
            }

            if is_terminal {
                break;
            }
        }

        session_id
    }

    fn session_event_span(
        session_id: SessionId,
        session_span: &Span,
        event: &MoqtSessionEvent,
    ) -> Span {
        match event {
            MoqtSessionEvent::PublishNamespace(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishNamespace",
                track_namespace = %handler.track_namespace(),
            ),
            MoqtSessionEvent::PublishNamespaceDone(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishNamespaceDone",
                track_namespace = %handler.track_namespace(),
            ),
            MoqtSessionEvent::SubscribeNamespace(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "SubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
            ),
            MoqtSessionEvent::UnsubscribeNamespace(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "UnsubscribeNamespace",
                track_namespace_prefix = %handler.track_namespace_prefix(),
            ),
            MoqtSessionEvent::Publish(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Publish",
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                track_alias = handler.track_alias(),
                group_order = ?handler._group_order(),
                content_exists = ?handler._content_exists(),
                forward = handler._forward(),
                delivery_timeout = ?handler._delivery_timeout(),
                max_cache_duration = ?handler._max_cache_duration(),
            ),
            MoqtSessionEvent::Subscribe(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Subscribe",
                subscribe_id = handler.subscribe_id(),
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
                subscriber_priority = handler._subscriber_priority(),
                group_order = ?handler._group_order(),
                forward = handler._forward(),
                filter_type = ?handler._filter_type(),
                max_cache_duration = ?handler._max_cache_duration(),
                delivery_timeout = ?handler._delivery_timeout(),
            ),
            MoqtSessionEvent::Unsubscribe(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Unsubscribe",
                subscribe_id = handler.subscribe_id(),
            ),
            MoqtSessionEvent::Disconnected() => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Disconnected",
            ),
            MoqtSessionEvent::Fetch(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "Fetch",
                request_id = handler.request_id(),
                fetch_params = ?handler.fetch_params(),
            ),
            MoqtSessionEvent::ProtocolViolation() => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "ProtocolViolation",
            ),
            MoqtSessionEvent::GoAway(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "GoAway",
                new_session_uri = %handler.new_session_uri(),
            ),
            MoqtSessionEvent::MaxRequestId(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "MaxRequestId",
                request_id = handler.request_id(),
            ),
            MoqtSessionEvent::RequestsBlocked(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "RequestsBlocked",
                maximum_request_id = handler.maximum_request_id(),
            ),
            MoqtSessionEvent::PublishNamespaceCancel(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishNamespaceCancel",
                track_namespace = %handler.track_namespace(),
                error_code = handler.error_code(),
                error_reason = %handler.error_reason(),
            ),
            MoqtSessionEvent::PublishDone(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "PublishDone",
                request_id = handler.request_id(),
                status_code = handler.status_code(),
                stream_count = handler.stream_count(),
                error_reason = %handler.error_reason(),
            ),
            MoqtSessionEvent::SubscribeUpdate(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "SubscribeUpdate",
                request_id = handler.request_id(),
                subscription_request_id = handler.subscription_request_id(),
                start_location = ?handler.start_location(),
                end_group = handler.end_group(),
                subscriber_priority = handler.subscriber_priority(),
                forward = handler.forward(),
            ),
            MoqtSessionEvent::FetchCancel(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "FetchCancel",
                request_id = handler.request_id(),
            ),
            MoqtSessionEvent::TrackStatus(handler) => tracing::info_span!(
                parent: session_span,
                "relay.session.event",
                session_id = %session_id,
                event = "TrackStatus",
                request_id = handler.request_id(),
                track_namespace = %handler.track_namespace(),
                track_name = %handler.track_name(),
            ),
        }
    }

    /// Idempotent: safe to call when the session is already absent.
    async fn cleanup_session(
        session_id: SessionId,
        local_pub_sub_directory: &InMemoryLocalPubSubDirectory,
        control_message_forwarder: &ControlMessageForwarder,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let removed = local_pub_sub_directory.remove_session(session_id);
        Self::cleanup_removed_session(
            session_id,
            removed,
            local_pub_sub_directory,
            control_message_forwarder,
            ingress_sender,
            route_registry,
            inter_relay_connection_manager,
        )
        .await;
        control_message_forwarder
            .repository
            .lock()
            .await
            .remove(session_id);
    }

    async fn cleanup_removed_session(
        removed_session_id: SessionId,
        removed: RemovedSessionSubscriptions,
        table: &InMemoryLocalPubSubDirectory,
        control_message_forwarder: &ControlMessageForwarder,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        for removed_downstream in removed.downstream_subscriptions {
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

                Self::stop_ingress_track(
                    ingress_sender,
                    removed_downstream.track_key,
                    removed_downstream.upstream_key.publisher_session_id,
                )
                .await;
            }
        }

        for track_key in removed.upstream_track_keys {
            Self::stop_ingress_track(ingress_sender, track_key, removed_session_id).await;
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
        publisher_session_id: SessionId,
    ) {
        if ingress_sender
            .send(IngressCommand::StopTrack {
                track_key: track_key.clone(),
                publisher_session_id,
            })
            .await
            .is_err()
        {
            tracing::debug!(%track_key, "failed to send ingress stop request");
        }
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        tracing::info!("Manager dropped.");
        self.relay_session_event_handler.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;

    use super::{EventHandler, WorkerDeps};
    use crate::modules::{
        auth::{
            test_support::{StubOutcome, StubVerifier},
            verified_token::VerifiedToken,
        },
        control_message_forwarder::ControlMessageForwarder,
        core::{
            mocks::{RecordedControlMessages, mock_new_session},
            session_event::MoqtSessionEvent,
        },
        inter_relay::InterRelayConnectionManager,
        relay::{
            cache::store::TrackCacheStore, egress::coordinator::EgressCommand,
            ingress::ingress_coordinator::IngressCommand,
        },
        route_registry::{NoopRelayRouteRegistry, RelayRouteRegistry},
        sequences::{
            tables::hashmap_table::InMemoryLocalPubSubDirectory,
            upstream_serializer::UpstreamCreationSerializer,
        },
        session_event::{EventKind, SessionEvent},
        session_repository::SessionRepository,
        types::{SessionId, TrackKey},
        upstream_publisher_resolver::UpstreamPublisherResolver,
    };

    const WAIT_TIMEOUT: Duration = Duration::from_secs(3);

    struct RunningEventHandler {
        _event_handler: EventHandler,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        event_sender: mpsc::UnboundedSender<SessionEvent>,
        _ingress_receiver: mpsc::Receiver<IngressCommand>,
        _egress_receiver: mpsc::Receiver<EgressCommand>,
        cache_store: Arc<TrackCacheStore>,
        cache_store_references_without_workers: usize,
    }

    impl RunningEventHandler {
        fn start() -> Self {
            let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
            let (event_sender, event_receiver) = mpsc::unbounded_channel();
            let (ingress_sender, ingress_receiver) = mpsc::channel(64);
            let (egress_sender, egress_receiver) = mpsc::channel(64);
            let route_registry: Arc<dyn RelayRouteRegistry> = Arc::new(NoopRelayRouteRegistry);
            let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
                repo.clone(),
                event_sender.clone(),
                String::new(),
            ));
            let upstream_publisher_resolver = Arc::new(UpstreamPublisherResolver::new(
                route_registry.clone(),
                inter_relay_connection_manager.clone(),
            ));
            let cache_store = Arc::new(TrackCacheStore::new());
            let event_handler = EventHandler::run(
                event_receiver,
                WorkerDeps {
                    control_message_forwarder: ControlMessageForwarder {
                        repository: repo.clone(),
                    },
                    repo: repo.clone(),
                    relay_event_sender: event_sender.clone(),
                    local_pub_sub_directory: Arc::new(InMemoryLocalPubSubDirectory::new()),
                    ingress_sender,
                    egress_sender,
                    route_registry,
                    inter_relay_connection_manager,
                    upstream_publisher_resolver,
                    cache_store: cache_store.clone(),
                    upstream_serializer: UpstreamCreationSerializer::new(),
                    token_verifier: Arc::new(StubVerifier(StubOutcome::Unauthorized)),
                },
            );
            let cache_store_references_without_workers = Arc::strong_count(&cache_store);
            Self {
                _event_handler: event_handler,
                repo,
                event_sender,
                _ingress_receiver: ingress_receiver,
                _egress_receiver: egress_receiver,
                cache_store,
                cache_store_references_without_workers,
            }
        }

        async fn register_session(&self, session_id: SessionId) -> RecordedControlMessages {
            let (new_session, recorded) =
                mock_new_session(session_id, VerifiedToken::full_access());
            self.repo
                .lock()
                .await
                .add(new_session, self.event_sender.clone())
                .await;
            recorded
        }

        fn send(&self, event: SessionEvent) {
            self.event_sender
                .send(event)
                .expect("event handler should accept events");
        }

        fn live_worker_count(&self) -> usize {
            Arc::strong_count(&self.cache_store) - self.cache_store_references_without_workers
        }

        async fn wait_for_live_workers(&self, expected: usize) {
            tokio::time::timeout(WAIT_TIMEOUT, async {
                while self.live_worker_count() != expected {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "expected {expected} live session workers, found {}",
                    self.live_worker_count()
                )
            });
        }

        async fn wait_until_reader_dispatched_earlier_events(
            &self,
            bystander_session_id: SessionId,
            bystander: &RecordedControlMessages,
        ) {
            self.send(SessionEvent::protocol_violation_detected(
                bystander_session_id,
                "barrier".to_string(),
            ));
            tokio::time::timeout(WAIT_TIMEOUT, async {
                while bystander.closes().is_empty() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("bystander session should be closed");
        }
    }

    #[tokio::test]
    async fn late_events_for_departed_or_unknown_sessions_start_no_worker() {
        // Arrange
        let handler = RunningEventHandler::start();
        handler.register_session(5).await;
        let bystander = handler.register_session(7).await;
        handler.send(SessionEvent {
            session_id: 5,
            kind: EventKind::FromSession(MoqtSessionEvent::Disconnected()),
        });
        handler.wait_for_live_workers(1).await;

        // Act
        handler.send(SessionEvent::malformed_track_detected(
            5,
            TrackKey::new("ns", "track"),
        ));
        handler.send(SessionEvent::malformed_track_detected(
            6,
            TrackKey::new("ns", "track"),
        ));
        handler
            .wait_until_reader_dispatched_earlier_events(7, &bystander)
            .await;

        // Assert
        assert_eq!(handler.live_worker_count(), 1);
    }
}
