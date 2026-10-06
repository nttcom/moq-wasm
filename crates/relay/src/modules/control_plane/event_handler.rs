mod session_cleanup;
mod session_event_span;
mod session_worker;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use self::session_worker::SessionWorker;
use crate::modules::{
    auth::token_verifier::TokenVerifier,
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::RelayRouteRegistry,
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        upstream_creation_serializer::UpstreamCreationSerializer,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{
        cache::store::TrackCacheStore, egress::coordinator::EgressCommand,
        ingress::ingress_coordinator::IngressCommand,
    },
    domain::{pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId},
    session::{
        session_event::{EventKind, SessionEvent},
        session_repository::SessionRepository,
    },
};

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
                                workers.spawn(SessionWorker::run(session_id, rx, deps.clone()));
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
        cascading::{
            inter_relay_connection_manager::InterRelayConnectionManager,
            route_registry::{NoopRelayRouteRegistry, RelayRouteRegistry},
        },
        control_plane::{
            control_message_forwarder::ControlMessageForwarder,
            upstream_creation_serializer::UpstreamCreationSerializer,
            upstream_publisher_resolver::UpstreamPublisherResolver,
        },
        data_plane::{
            cache::store::TrackCacheStore, egress::coordinator::EgressCommand,
            ingress::ingress_coordinator::IngressCommand,
        },
        domain::{
            pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId,
            session_peer::SessionPeer, track_key::TrackKey,
        },
        session::{
            moqt_session_event::MoqtSessionEvent,
            session_event::{EventKind, SessionEvent},
            session_repository::SessionRepository,
        },
        test_support::mock_session::{
            MockFetchHandler, MockPublishHandler, MockPublishNamespaceHandler,
            RecordedControlMessages, mock_new_session,
        },
    };

    const WAIT_TIMEOUT: Duration = Duration::from_secs(3);

    struct RunningEventHandler {
        _event_handler: EventHandler,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        event_sender: mpsc::UnboundedSender<SessionEvent>,
        _ingress_receiver: mpsc::Receiver<IngressCommand>,
        _egress_receiver: mpsc::Receiver<EgressCommand>,
        local_pub_sub_directory: Arc<InMemoryLocalPubSubDirectory>,
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
            let local_pub_sub_directory = Arc::new(InMemoryLocalPubSubDirectory::new());
            let event_handler = EventHandler::run(
                event_receiver,
                WorkerDeps {
                    control_message_forwarder: ControlMessageForwarder {
                        repository: repo.clone(),
                    },
                    repo: repo.clone(),
                    relay_event_sender: event_sender.clone(),
                    local_pub_sub_directory: local_pub_sub_directory.clone(),
                    ingress_sender,
                    egress_sender,
                    route_registry,
                    inter_relay_connection_manager,
                    upstream_publisher_resolver,
                    cache_store: cache_store.clone(),
                    upstream_serializer: UpstreamCreationSerializer::default(),
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
                local_pub_sub_directory,
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

        async fn register_publisher(&self, session_id: SessionId, track_namespace: &str) {
            self.register_session(session_id).await;
            self.local_pub_sub_directory.register_publish_namespace(
                session_id,
                track_namespace.to_string(),
                SessionPeer::Client,
            );
        }

        async fn register_namespace_subscriber(
            &self,
            session_id: SessionId,
            track_namespace_prefix: &str,
        ) -> RecordedControlMessages {
            let recorded = self.register_session(session_id).await;
            self.local_pub_sub_directory.register_subscribe_namespace(
                session_id,
                track_namespace_prefix.to_string(),
                SessionPeer::Client,
            );
            recorded
        }

        fn send_publish_namespace(&self, session_id: SessionId, track_namespace: &str) {
            self.send(SessionEvent {
                session_id,
                kind: EventKind::FromSession(MoqtSessionEvent::PublishNamespace(Box::new(
                    MockPublishNamespaceHandler::new(track_namespace),
                ))),
            });
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
            wait_for_close(bystander)
                .await
                .expect("bystander session should be closed");
        }
    }

    async fn wait_for_close(
        recorded: &RecordedControlMessages,
    ) -> Result<(), tokio::time::error::Elapsed> {
        tokio::time::timeout(WAIT_TIMEOUT, async {
            while recorded.closes().is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
    }

    async fn wait_for_publish_namespaces(
        recorded: &RecordedControlMessages,
        expected: &[&str],
    ) -> Result<(), tokio::time::error::Elapsed> {
        tokio::time::timeout(WAIT_TIMEOUT, async {
            while recorded.publish_namespaces() != expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
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

    #[tokio::test]
    async fn an_unanswered_upstream_fetch_does_not_hold_later_events_of_the_session() {
        // Arrange
        let handler = RunningEventHandler::start();
        handler.register_publisher(1, "ns").await;
        let subscriber = handler.register_session(2).await;
        handler.send(SessionEvent {
            session_id: 2,
            kind: EventKind::FromSession(MoqtSessionEvent::Fetch(Box::new(MockFetchHandler))),
        });

        // Act
        handler.send(SessionEvent::protocol_violation_detected(
            2,
            "event after the fetch".to_string(),
        ));

        // Assert
        wait_for_close(&subscriber).await.expect(
            "the event after the FETCH should be handled while the upstream FETCH is pending",
        );
    }

    #[tokio::test]
    async fn an_unanswered_forwarded_publish_does_not_hold_later_events_of_the_publisher() {
        // Arrange
        let handler = RunningEventHandler::start();
        handler.register_namespace_subscriber(2, "ns").await;
        let publisher = handler.register_session(1).await;
        handler.send(SessionEvent {
            session_id: 1,
            kind: EventKind::FromSession(MoqtSessionEvent::Publish(Box::new(
                MockPublishHandler::new("ns", "track", 0),
            ))),
        });

        // Act
        handler.send(SessionEvent::protocol_violation_detected(
            1,
            "event after the PUBLISH".to_string(),
        ));

        // Assert
        wait_for_close(&publisher).await.expect(
            "the event after the PUBLISH should be handled while the forwarded PUBLISH is unanswered",
        );
    }

    #[tokio::test]
    async fn an_unanswered_publish_namespace_does_not_hold_later_events_of_the_publisher() {
        // Arrange
        let handler = RunningEventHandler::start();
        handler.register_namespace_subscriber(2, "ns").await;
        let publisher = handler.register_session(1).await;
        handler.send_publish_namespace(1, "ns/a");

        // Act
        handler.send(SessionEvent::protocol_violation_detected(
            1,
            "event after the publish namespace".to_string(),
        ));

        // Assert
        wait_for_close(&publisher).await.expect(
            "the event after PUBLISH_NAMESPACE should be handled while the subscriber has not answered",
        );
    }

    #[tokio::test]
    async fn publish_namespace_reaches_every_namespace_subscriber_while_none_answers() {
        // Arrange
        let handler = RunningEventHandler::start();
        let first_subscriber = handler.register_namespace_subscriber(2, "ns").await;
        let second_subscriber = handler.register_namespace_subscriber(3, "ns").await;
        handler.register_session(1).await;

        // Act
        handler.send_publish_namespace(1, "ns/a");

        // Assert
        wait_for_publish_namespaces(&first_subscriber, &["ns/a"])
            .await
            .expect("the first subscriber should receive PUBLISH_NAMESPACE");
        wait_for_publish_namespaces(&second_subscriber, &["ns/a"])
            .await
            .expect("the second subscriber should receive PUBLISH_NAMESPACE");
    }

    #[tokio::test]
    async fn publish_namespace_is_echoed_to_its_publisher_subscribed_to_the_prefix() {
        // Arrange
        let handler = RunningEventHandler::start();
        let publisher = handler.register_namespace_subscriber(1, "anon").await;

        // Act
        handler.send_publish_namespace(1, "anon/x/y");

        // Assert: draft-14 §6.1 includes echoing PUBLISH_NAMESPACE back to the endpoint that sent it
        wait_for_publish_namespaces(&publisher, &["anon/x/y"])
            .await
            .expect("the publisher should receive its own PUBLISH_NAMESPACE");
    }
}
