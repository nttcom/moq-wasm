use std::sync::Arc;

use crate::{
    config::DEFAULT_FETCH_FILL_TIMEOUT,
    modules::{
        auth::test_support::{StubOutcome, StubVerifier},
        cascading::{
            inter_relay_connection_manager::InterRelayConnectionManager,
            route_registry::{NoopRelayRouteRegistry, RelayRouteRegistry},
        },
        control_plane::{
            control_message_forwarder::ControlMessageForwarder,
            session_workers::{SessionWorkers, WorkerDeps},
            upstream_publisher_resolver::UpstreamPublisherResolver,
        },
        data_plane::cache::store::TrackCacheStore,
        directory::InMemoryLocalPubSubDirectory,
        session::session_repository::SessionRepository,
        test_support::{
            directory_fixtures::unused_inter_relay_client_config, mock_session::session_repository,
        },
    },
};

/// The session workers as the relay runs them: a session added to `repo`
/// gets its worker.
pub(crate) struct RunningSessionWorkers {
    _session_workers: SessionWorkers,
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) local_pub_sub_directory: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) cache_store_references_without_workers: usize,
}

impl RunningSessionWorkers {
    pub(crate) fn start() -> Self {
        Self::start_with_route_registry(Arc::new(NoopRelayRouteRegistry))
    }

    pub(crate) fn start_with_route_registry(route_registry: Arc<dyn RelayRouteRegistry>) -> Self {
        let (repo, session_added_receiver) = session_repository();
        let repo = Arc::new(tokio::sync::Mutex::new(repo));
        let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
            repo.clone(),
            unused_inter_relay_client_config(),
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
                namespace_routes: upstream_publisher_resolver.namespace_routes.clone(),
                route_registry,
                inter_relay_connection_manager: inter_relay_connection_manager.clone(),
                upstream_publisher_resolver,
                cache_store: cache_store.clone(),
                token_verifier: Arc::new(StubVerifier(StubOutcome::Unauthorized)),
                fetch_fill_timeout: DEFAULT_FETCH_FILL_TIMEOUT,
            },
        );
        let cache_store_references_without_workers = Arc::strong_count(&cache_store);
        Self {
            _session_workers: session_workers,
            repo,
            local_pub_sub_directory,
            inter_relay_connection_manager,
            cache_store,
            cache_store_references_without_workers,
        }
    }
}
