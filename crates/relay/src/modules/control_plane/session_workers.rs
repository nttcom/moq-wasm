mod session_cleanup;
mod session_event_span;
mod session_worker;

use std::sync::Arc;
use std::time::Duration;
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
    data_plane::cache::store::TrackCacheStore,
    directory::InMemoryLocalPubSubDirectory,
    domain::session_id::SessionId,
    session::session_repository::{SessionAdded, SessionRepository},
};

/// Runs one `SessionWorker` per session added to the repository and reaps
/// the workers as their sessions end.
pub(crate) struct SessionWorkers {
    join_handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct WorkerDeps {
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) control_message_forwarder: ControlMessageForwarder,
    pub(crate) local_pub_sub_directory: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) route_registry: Arc<dyn RelayRouteRegistry>,
    pub(crate) inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    pub(crate) upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) upstream_serializer: UpstreamCreationSerializer,
    pub(crate) token_verifier: Arc<dyn TokenVerifier>,
    pub(crate) fetch_fill_timeout: Duration,
}

impl SessionWorkers {
    pub(crate) fn run(
        mut session_added_receiver: mpsc::UnboundedReceiver<SessionAdded>,
        deps: WorkerDeps,
    ) -> Self {
        let join_handle = tokio::task::Builder::new()
            .name("Relay Session Workers")
            .spawn(async move {
                let mut workers: tokio::task::JoinSet<SessionId> = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        result = workers.join_next(), if !workers.is_empty() => {
                            match result {
                                Some(Ok(session_id)) => {
                                    tracing::debug!(session_id, "session worker exited");
                                }
                                Some(Err(error)) => {
                                    tracing::error!(?error, "session worker panicked");
                                }
                                None => {}
                            }
                        }
                        session_added = session_added_receiver.recv() => {
                            let Some(SessionAdded { session_id, mailbox_receiver }) = session_added else {
                                tracing::debug!("session registration channel closed; awaiting the workers");
                                break;
                            };
                            workers.spawn(SessionWorker::run(session_id, mailbox_receiver, deps.clone()));
                        }
                    }
                }
                while workers.join_next().await.is_some() {}
            })
            .unwrap();
        Self { join_handle }
    }
}

impl Drop for SessionWorkers {
    fn drop(&mut self) {
        tracing::debug!("session workers dropped");
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use crate::config::DEFAULT_FETCH_FILL_TIMEOUT;
    use std::sync::Arc;
    use std::time::Duration;

    use super::{SessionWorkers, WorkerDeps};
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
        data_plane::cache::store::TrackCacheStore,
        directory::InMemoryLocalPubSubDirectory,
        domain::{session_id::SessionId, session_peer::SessionPeer},
        session::{moqt_session_event::MoqtSessionEvent, session_repository::SessionRepository},
        test_support::mock_session::{
            MockFetchHandler, MockPublishHandler, MockPublishNamespaceHandler,
            RecordedControlMessages, mock_new_session, session_repository,
        },
    };

    const WAIT_TIMEOUT: Duration = Duration::from_secs(3);

    struct RunningSessionWorkers {
        _session_workers: SessionWorkers,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        local_pub_sub_directory: Arc<InMemoryLocalPubSubDirectory>,
        cache_store: Arc<TrackCacheStore>,
        cache_store_references_without_workers: usize,
    }

    impl RunningSessionWorkers {
        fn start() -> Self {
            let (repo, session_added_receiver) = session_repository();
            let repo = Arc::new(tokio::sync::Mutex::new(repo));
            let route_registry: Arc<dyn RelayRouteRegistry> = Arc::new(NoopRelayRouteRegistry);
            let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
                repo.clone(),
                String::new(),
            ));
            let upstream_publisher_resolver = Arc::new(UpstreamPublisherResolver::new(
                route_registry.clone(),
                inter_relay_connection_manager.clone(),
            ));
            let cache_store = Arc::new(TrackCacheStore::new());
            let local_pub_sub_directory = Arc::new(InMemoryLocalPubSubDirectory::new());
            let session_workers = SessionWorkers::run(
                session_added_receiver,
                WorkerDeps {
                    control_message_forwarder: ControlMessageForwarder {
                        repository: repo.clone(),
                    },
                    repo: repo.clone(),
                    local_pub_sub_directory: local_pub_sub_directory.clone(),
                    route_registry,
                    inter_relay_connection_manager,
                    upstream_publisher_resolver,
                    cache_store: cache_store.clone(),
                    upstream_serializer: UpstreamCreationSerializer::default(),
                    token_verifier: Arc::new(StubVerifier(StubOutcome::Unauthorized)),
                    fetch_fill_timeout: DEFAULT_FETCH_FILL_TIMEOUT,
                },
            );
            let cache_store_references_without_workers = Arc::strong_count(&cache_store);
            Self {
                _session_workers: session_workers,
                repo,
                local_pub_sub_directory,
                cache_store,
                cache_store_references_without_workers,
            }
        }

        async fn register_session(&self, session_id: SessionId) -> RecordedControlMessages {
            let (new_session, recorded) =
                mock_new_session(session_id, VerifiedToken::full_access());
            self.repo.lock().await.add(new_session);
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

        /// Every worker holds a clone of `WorkerDeps`, so the cache store's
        /// extra references count the live workers.
        fn live_worker_count(&self) -> usize {
            Arc::strong_count(&self.cache_store) - self.cache_store_references_without_workers
        }

        async fn session_leaves(
            &self,
            session_id: SessionId,
        ) -> Result<(), tokio::time::error::Elapsed> {
            tokio::time::timeout(WAIT_TIMEOUT, async {
                while self.repo.lock().await.mailbox(session_id).is_some() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
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
    }

    fn publish_namespace(track_namespace: &str) -> MoqtSessionEvent {
        MoqtSessionEvent::PublishNamespace(Box::new(MockPublishNamespaceHandler::new(
            track_namespace,
        )))
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
    async fn a_disconnected_session_leaves_the_repository_with_its_worker() {
        // Arrange
        let workers = RunningSessionWorkers::start();
        let session = workers.register_session(5).await;
        workers.wait_for_live_workers(1).await;

        // Act
        session.send_event(MoqtSessionEvent::Disconnected());
        workers.wait_for_live_workers(0).await;

        // Assert
        assert!(workers.repo.lock().await.mailbox(5).is_none());
    }

    #[tokio::test]
    async fn a_failing_event_stream_cleans_the_session_up() {
        // Arrange
        let workers = RunningSessionWorkers::start();
        let session = workers.register_session(5).await;
        workers.wait_for_live_workers(1).await;

        // Act
        session.fail_event_stream();
        workers.wait_for_live_workers(0).await;

        // Assert
        assert!(workers.repo.lock().await.mailbox(5).is_none());
    }

    #[tokio::test]
    async fn an_unanswered_upstream_fetch_does_not_hold_later_events_of_the_session() {
        // Arrange
        let workers = RunningSessionWorkers::start();
        workers.register_publisher(1, "ns").await;
        let subscriber = workers.register_session(2).await;
        workers.wait_for_live_workers(2).await;
        subscriber.send_event(MoqtSessionEvent::Fetch(Box::new(MockFetchHandler)));

        // Act
        subscriber.send_event(MoqtSessionEvent::Disconnected());

        // Assert
        workers.session_leaves(2).await.expect(
            "the event after the FETCH should be handled while the upstream FETCH is pending",
        );
    }

    #[tokio::test]
    async fn an_unanswered_forwarded_publish_does_not_hold_later_events_of_the_publisher() {
        // Arrange
        let workers = RunningSessionWorkers::start();
        workers.register_namespace_subscriber(2, "ns").await;
        let publisher = workers.register_session(1).await;
        workers.wait_for_live_workers(2).await;
        publisher.send_event(MoqtSessionEvent::Publish(Box::new(
            MockPublishHandler::new("ns", "track", 0),
        )));

        // Act
        publisher.send_event(MoqtSessionEvent::Disconnected());

        // Assert
        workers.session_leaves(1).await.expect(
            "the event after the PUBLISH should be handled while the forwarded PUBLISH is unanswered",
        );
    }

    #[tokio::test]
    async fn an_unanswered_publish_namespace_does_not_hold_later_events_of_the_publisher() {
        // Arrange
        let workers = RunningSessionWorkers::start();
        workers.register_namespace_subscriber(2, "ns").await;
        let publisher = workers.register_session(1).await;
        workers.wait_for_live_workers(2).await;
        publisher.send_event(publish_namespace("ns/a"));

        // Act
        publisher.send_event(MoqtSessionEvent::Disconnected());

        // Assert
        workers.session_leaves(1).await.expect(
            "the event after PUBLISH_NAMESPACE should be handled while the subscriber has not answered",
        );
    }

    #[tokio::test]
    async fn publish_namespace_reaches_every_namespace_subscriber_while_none_answers() {
        // Arrange
        let workers = RunningSessionWorkers::start();
        let first_subscriber = workers.register_namespace_subscriber(2, "ns").await;
        let second_subscriber = workers.register_namespace_subscriber(3, "ns").await;
        let publisher = workers.register_session(1).await;

        // Act
        publisher.send_event(publish_namespace("ns/a"));

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
        let workers = RunningSessionWorkers::start();
        let publisher = workers.register_namespace_subscriber(1, "anon").await;

        // Act
        publisher.send_event(publish_namespace("anon/x/y"));

        // Assert: draft-14 §6.1 includes echoing PUBLISH_NAMESPACE back to the endpoint that sent it
        wait_for_publish_namespaces(&publisher, &["anon/x/y"])
            .await
            .expect("the publisher should receive its own PUBLISH_NAMESPACE");
    }
}
