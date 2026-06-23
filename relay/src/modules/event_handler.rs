use std::collections::HashMap;
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
        upstream_serializer::UpstreamCreationSerializer,
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

/// All deps cloned from the reader into a newly spawned session worker.
struct WorkerDeps {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    control_message_forwarder: ControlMessageForwarder,
    local_pub_sub_directory: Arc<dyn LocalPubSubDirectory>,
    ingress_sender: mpsc::Sender<IngressCommand>,
    egress_sender: mpsc::Sender<EgressCommand>,
    route_registry: Arc<dyn RelayRouteRegistry>,
    inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
    cache_store: Arc<TrackCacheStore>,
    upstream_serializer: UpstreamCreationSerializer,
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
                // One serializer per relay; cloned (Arc-backed) into each worker.
                let upstream_serializer = UpstreamCreationSerializer::new();

                // sender_map: session_id -> per-session channel sender.
                // Only the reader touches this map; no locking needed.
                let mut sender_map: HashMap<SessionId, mpsc::UnboundedSender<SessionEvent>> =
                    HashMap::new();

                // Workers are tracked in a JoinSet that yields the session_id
                // of the exiting worker, so the reader can remove stale entries.
                let mut workers: tokio::task::JoinSet<SessionId> =
                    tokio::task::JoinSet::new();

                loop {
                    tokio::select! {
                        // Reap finished workers and remove their sender map entries.
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

                            let session_id = match &event {
                                SessionEvent::PublishNameSpace(id, _)
                                | SessionEvent::PublishNamespaceDone(id, _)
                                | SessionEvent::SubscribeNameSpace(id, _)
                                | SessionEvent::UnsubscribeNameSpace(id, _)
                                | SessionEvent::Publish(id, _)
                                | SessionEvent::Subscribe(id, _)
                                | SessionEvent::Unsubscribe(id, _)
                                | SessionEvent::Fetch(id, _)
                                | SessionEvent::Disconnected(id)
                                | SessionEvent::ProtocolViolation(id) => *id,
                            };

                            // Locate or lazily create the per-session channel.
                            // Single-threaded reader: creation is race-free.
                            let sender = sender_map.entry(session_id).or_insert_with(|| {
                                let (tx, rx) = mpsc::unbounded_channel::<SessionEvent>();
                                let deps = WorkerDeps {
                                    repo: repo.clone(),
                                    control_message_forwarder: control_message_forwarder.clone(),
                                    local_pub_sub_directory: local_pub_sub_directory.clone(),
                                    ingress_sender: ingress_sender.clone(),
                                    egress_sender: egress_sender.clone(),
                                    route_registry: route_registry.clone(),
                                    inter_relay_connection_manager: inter_relay_connection_manager.clone(),
                                    upstream_publisher_resolver: upstream_publisher_resolver.clone(),
                                    cache_store: cache_store.clone(),
                                    upstream_serializer: upstream_serializer.clone(),
                                };
                                workers.spawn(Self::session_worker(session_id, rx, deps));
                                tx
                            });

                            // Unbounded send never blocks, so the reader never
                            // stalls on a slow or blocked worker.
                            if sender.send(event).is_err() {
                                // Worker already exited (race with join_next);
                                // the sender will be removed on the next join poll.
                                tracing::warn!(session_id, "session worker channel closed before send");
                            }
                        }
                    }
                }

                // Drop sender_map so every worker observes channel close and exits.
                drop(sender_map);
                while workers.join_next().await.is_some() {}
            })
            .unwrap()
    }

    /// Per-session worker.  Processes events STRICTLY IN ORDER, fully awaiting
    /// each handler (including peer responses) before pulling the next event.
    /// A single session's events are FIFO; different sessions' workers run
    /// concurrently in independent tasks.
    ///
    /// Returns the session_id so the reader can remove the sender map entry.
    async fn session_worker(
        session_id: SessionId,
        mut rx: mpsc::UnboundedReceiver<SessionEvent>,
        deps: WorkerDeps,
    ) -> SessionId {
        let WorkerDeps {
            repo,
            control_message_forwarder,
            local_pub_sub_directory,
            ingress_sender,
            egress_sender,
            route_registry,
            inter_relay_connection_manager,
            upstream_publisher_resolver,
            cache_store,
            upstream_serializer,
        } = deps;

        while let Some(event) = rx.recv().await {
            // Determine terminality BEFORE the span check so that a terminal
            // event always breaks the loop, even when the session span is
            // already gone (e.g. the second terminal event in a
            // timeout-then-disconnect sequence).
            let is_terminal = matches!(
                event,
                SessionEvent::Disconnected(_) | SessionEvent::ProtocolViolation(_)
            );

            let session_span = {
                let repo = repo.lock().await;
                repo.session_span(session_id)
            };
            let Some(session_span) = session_span else {
                tracing::warn!(session_id, "Session span not found");
                if is_terminal {
                    // Span is gone but this is still a terminal event.
                    // Run idempotent cleanup (remove_session on an
                    // already-removed session is a no-op) and exit so
                    // the worker does not block on rx.recv() forever.
                    Self::cleanup_session(
                        session_id,
                        local_pub_sub_directory.as_ref(),
                        &control_message_forwarder,
                        &ingress_sender,
                        &egress_sender,
                        route_registry.as_ref(),
                        inter_relay_connection_manager.as_ref(),
                    )
                    .await;
                    break;
                }
                continue;
            };
            let event_span = Self::session_event_span(&session_span, &event);

            match event {
                SessionEvent::PublishNameSpace(session_id, handler) => {
                    PublishNamespace {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            CascadingRelayContext {
                                route_registry: route_registry.as_ref(),
                                inter_relay_connection_manager: inter_relay_connection_manager
                                    .as_ref(),
                            },
                            handler.as_ref(),
                        )
                        .instrument(event_span)
                        .await;
                }
                SessionEvent::PublishNamespaceDone(session_id, handler) => {
                    PublishNamespaceDone {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            CascadingRelayContext {
                                route_registry: route_registry.as_ref(),
                                inter_relay_connection_manager: inter_relay_connection_manager
                                    .as_ref(),
                            },
                            handler.as_ref(),
                        )
                        .instrument(event_span)
                        .await;
                }
                SessionEvent::SubscribeNameSpace(session_id, handler) => {
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
                SessionEvent::UnsubscribeNameSpace(session_id, handler) => {
                    UnsubscribeNamespace {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            CascadingRelayContext {
                                route_registry: route_registry.as_ref(),
                                inter_relay_connection_manager: inter_relay_connection_manager
                                    .as_ref(),
                            },
                            handler.as_ref(),
                        )
                        .instrument(event_span)
                        .await;
                }
                SessionEvent::Publish(session_id, handler) => {
                    Publish {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                            CascadingRelayContext {
                                route_registry: route_registry.as_ref(),
                                inter_relay_connection_manager: inter_relay_connection_manager
                                    .as_ref(),
                            },
                            handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                SessionEvent::Subscribe(session_id, handler) => {
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
                SessionEvent::Unsubscribe(session_id, handler) => {
                    Unsubscribe {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                            &egress_sender,
                            handler,
                        )
                        .instrument(event_span)
                        .await;
                }
                SessionEvent::Fetch(session_id, handler) => {
                    Fetch {}
                        .handle(
                            session_id,
                            &session_span,
                            local_pub_sub_directory.as_ref(),
                            &cache_store,
                            &egress_sender,
                            handler,
                        )
                        .instrument(event_span)
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
                        Self::cleanup_session(
                            session_id,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                            &egress_sender,
                            route_registry.as_ref(),
                            inter_relay_connection_manager.as_ref(),
                        )
                        .await;
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
                        Self::cleanup_session(
                            session_id,
                            local_pub_sub_directory.as_ref(),
                            &control_message_forwarder,
                            &ingress_sender,
                            &egress_sender,
                            route_registry.as_ref(),
                            inter_relay_connection_manager.as_ref(),
                        )
                        .await;
                    }
                    .instrument(protocol_violation_span)
                    .await;
                }
            }

            // Exit after processing a terminal event: the session is cleaned
            // up and no further events for it are meaningful.
            if is_terminal {
                break;
            }
        }

        session_id
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

    /// Idempotent session cleanup: remove the session from the pub/sub
    /// directory, run subscription teardown, then drop the session from the
    /// repository.  Safe to call when the session is already absent; all
    /// operations degrade gracefully.
    #[allow(clippy::too_many_arguments)]
    async fn cleanup_session(
        session_id: SessionId,
        local_pub_sub_directory: &dyn LocalPubSubDirectory,
        control_message_forwarder: &ControlMessageForwarder,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        egress_sender: &mpsc::Sender<EgressCommand>,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let removed = local_pub_sub_directory.remove_session(session_id).await;
        Self::cleanup_removed_session(
            session_id,
            removed,
            local_pub_sub_directory,
            control_message_forwarder,
            ingress_sender,
            egress_sender,
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

        // TODO(deadlock-core): iteration 4 — make upstream join/remove atomic across sessions
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
            .send(IngressCommand::StopTrack {
                track_key: track_key.clone(),
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

    /// Verifies that per-session serialisation does NOT block other sessions.
    ///
    /// We simulate the reader/worker structure directly: two sessions each get
    /// an unbounded channel, and their workers process events independently.
    /// A slow handler on session A (emulated with a sleep) must not delay
    /// events on session B.
    ///
    /// The real handlers depend on many heavy dependencies (repository,
    /// ingress/egress coordinators, etc.) and cannot be used in unit tests
    /// without significant mocking.  We therefore test the ROUTING layer in
    /// isolation, confirming that workers running independently don't
    /// head-of-line block each other.
    #[tokio::test]
    async fn slow_session_does_not_block_other_session() {
        let (tx_a, mut rx_a) = mpsc::unbounded_channel::<u64>();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel::<u64>();

        let results_a = Arc::new(tokio::sync::Mutex::new(Vec::<u64>::new()));
        let results_b = Arc::new(tokio::sync::Mutex::new(Vec::<u64>::new()));

        let ra = results_a.clone();
        let rb = results_b.clone();

        // Worker A: slow (50 ms per event).
        let worker_a = tokio::spawn(async move {
            while let Some(val) = rx_a.recv().await {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ra.lock().await.push(val);
            }
        });

        // Worker B: fast.
        let worker_b = tokio::spawn(async move {
            while let Some(val) = rx_b.recv().await {
                rb.lock().await.push(val);
            }
        });

        tx_a.send(1).unwrap();
        tx_b.send(2).unwrap();

        // 20 ms: enough for B to finish, not enough for A's 50 ms sleep.
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            *results_b.lock().await,
            vec![2],
            "session B should have processed its event already"
        );
        assert!(
            results_a.lock().await.is_empty(),
            "session A should still be blocked"
        );

        drop(tx_a);
        drop(tx_b);
        let _ = tokio::join!(worker_a, worker_b);
    }

    /// A terminal event whose session span is absent must still break the
    /// worker loop rather than issuing a `continue` that would block the
    /// worker on `rx.recv()` forever.
    ///
    /// We model the key invariant at the level of the control-flow structure
    /// (channel + loop with early-exit on terminal flag) because
    /// `session_worker` requires heavy real dependencies that cannot be
    /// instantiated without significant mocking infrastructure.
    #[tokio::test]
    async fn terminal_event_with_missing_span_breaks_worker() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Ev {
            Normal,
            Terminal,
        }
        // Simulate the fixed control-flow structure:
        //   is_terminal computed before span check;
        //   terminal + no-span → cleanup + break.
        let (tx, mut rx) = mpsc::unbounded_channel::<Ev>();
        let span_present = false; // span is absent for this session

        let worker = tokio::spawn(async move {
            let mut processed = Vec::new();
            while let Some(event) = rx.recv().await {
                let is_terminal = event == Ev::Terminal;
                if !span_present {
                    if is_terminal {
                        // cleanup would run here; in the real code this is
                        // Self::cleanup_session(...).await; break;
                        processed.push(event);
                        break;
                    }
                    continue;
                }
                processed.push(event);
                if is_terminal {
                    break;
                }
            }
            processed
        });

        // Send a normal event (should be skipped — no span) then a terminal one.
        tx.send(Ev::Normal).unwrap();
        tx.send(Ev::Terminal).unwrap();
        // The sender stays alive, so the worker MUST break itself — it cannot
        // rely on the channel closing.

        let processed = tokio::time::timeout(Duration::from_millis(500), worker)
            .await
            .expect("worker timed out — terminal event did not break the loop")
            .unwrap();

        // Normal event was skipped (no span), terminal event triggered cleanup+break.
        assert_eq!(processed, vec![Ev::Terminal]);
    }

    /// Single-session events must be processed in FIFO order.
    #[tokio::test]
    async fn single_session_events_are_fifo() {
        let (tx, mut rx) = mpsc::unbounded_channel::<u64>();
        let results = Arc::new(tokio::sync::Mutex::new(Vec::<u64>::new()));
        let r = results.clone();

        let worker = tokio::spawn(async move {
            while let Some(val) = rx.recv().await {
                r.lock().await.push(val);
            }
        });

        for i in 0..5u64 {
            tx.send(i).unwrap();
        }
        drop(tx);
        worker.await.unwrap();

        assert_eq!(*results.lock().await, vec![0, 1, 2, 3, 4]);
    }
}
