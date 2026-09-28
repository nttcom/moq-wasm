use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::modules::{
    auth::token_verifier::TokenVerifier,
    control_message_forwarder::ControlMessageForwarder,
    event_handler::{EventHandler, WorkerDeps},
    inter_relay::InterRelayConnectionManager,
    relay::{
        cache::eviction_job::spawn_cache_eviction_job, egress::coordinator::EgressCoordinator,
        ingress::ingress_coordinator::IngressCoordinator,
    },
    route_registry::RelayRouteRegistry,
    sequences::{
        tables::{hashmap_table::InMemoryLocalPubSubDirectory, table::LocalPubSubDirectory},
        upstream_serializer::UpstreamCreationSerializer,
    },
    session_event::SessionEvent,
    session_repository::SessionRepository,
    upstream_publisher_resolver::UpstreamPublisherResolver,
};
use crate::relay_server::store::RelayStore;

pub(crate) struct CascadingDeps {
    pub(crate) route_registry: Arc<dyn RelayRouteRegistry>,
    pub(crate) relay_token: String,
}

pub(crate) struct RelayRuntime {
    _ingress: IngressCoordinator,
    _egress: EgressCoordinator,
    _manager: EventHandler,
    _evict_job: tokio::task::JoinHandle<()>,
}

impl RelayRuntime {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        store: &Arc<RelayStore>,
        cascading: CascadingDeps,
        token_verifier: Arc<dyn TokenVerifier>,
    ) -> (UnboundedSender<SessionEvent>, Self) {
        let CascadingDeps {
            route_registry,
            relay_token,
        } = cascading;
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<SessionEvent>();
        let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
            repo.clone(),
            sender.clone(),
            relay_token,
        ));
        let upstream_publisher_resolver = Arc::new(UpstreamPublisherResolver::new(
            route_registry.clone(),
            inter_relay_connection_manager.clone(),
        ));
        let ingress = IngressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.subgroup_opened_notifier_map.clone(),
            sender.clone(),
        );
        let egress = EgressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.subgroup_opened_notifier_map.clone(),
        );
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
                cache_store: store.cache_store.clone(),
                upstream_serializer: UpstreamCreationSerializer::new(),
                token_verifier,
            },
        );
        let evict_job = spawn_cache_eviction_job(
            store.cache_store.clone(),
            store.subgroup_opened_notifier_map.clone(),
        );
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
