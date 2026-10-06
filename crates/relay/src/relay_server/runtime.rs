use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::modules::{
    auth::token_verifier::TokenVerifier,
    control_message_forwarder::ControlMessageForwarder,
    data_plane::{
        cache::{eviction_job::spawn_cache_eviction_job, store::TrackCacheStore},
        egress::coordinator::EgressCoordinator,
        ingress::ingress_coordinator::IngressCoordinator,
    },
    event_handler::{EventHandler, WorkerDeps},
    inter_relay::InterRelayConnectionManager,
    route_registry::RelayRouteRegistry,
    sequences::{
        tables::hashmap_table::InMemoryLocalPubSubDirectory,
        upstream_serializer::UpstreamCreationSerializer,
    },
    session::{session_event::SessionEvent, session_repository::SessionRepository},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};

pub(crate) struct RelayRuntime {
    _ingress: IngressCoordinator,
    _egress: EgressCoordinator,
    _manager: EventHandler,
    _evict_job: tokio::task::JoinHandle<()>,
}

impl RelayRuntime {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        route_registry: Arc<dyn RelayRouteRegistry>,
        relay_token: String,
        token_verifier: Arc<dyn TokenVerifier>,
    ) -> (UnboundedSender<SessionEvent>, Self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<SessionEvent>();
        let cache_store = Arc::new(TrackCacheStore::new());
        let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
            repo.clone(),
            sender.clone(),
            relay_token,
        ));
        let upstream_publisher_resolver = Arc::new(UpstreamPublisherResolver::new(
            route_registry.clone(),
            inter_relay_connection_manager.clone(),
        ));
        let ingress = IngressCoordinator::new(repo.clone(), cache_store.clone(), sender.clone());
        let egress = EgressCoordinator::new(repo.clone(), cache_store.clone());
        let manager = EventHandler::run(
            receiver,
            WorkerDeps {
                control_message_forwarder: ControlMessageForwarder {
                    repository: repo.clone(),
                },
                repo,
                relay_event_sender: sender.clone(),
                local_pub_sub_directory: Arc::new(InMemoryLocalPubSubDirectory::new()),
                ingress_sender: ingress.sender(),
                egress_sender: egress.sender(),
                route_registry,
                inter_relay_connection_manager,
                upstream_publisher_resolver,
                cache_store: cache_store.clone(),
                upstream_serializer: UpstreamCreationSerializer::default(),
                token_verifier,
            },
        );
        let evict_job = spawn_cache_eviction_job(cache_store.clone());
        (
            sender,
            Self {
                _ingress: ingress,
                _egress: egress,
                _manager: manager,
                _evict_job: evict_job,
            },
        )
    }
}
