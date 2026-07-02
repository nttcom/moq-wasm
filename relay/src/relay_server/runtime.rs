use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::modules::{
    event_handler::EventHandler,
    inter_relay::InterRelayConnectionManager,
    relay::{
        cache::eviction_job::spawn_cache_eviction_job, egress::coordinator::EgressCoordinator,
        ingress::ingress_coordinator::IngressCoordinator,
    },
    route_registry::RelayRouteRegistry,
    session_event::SessionEvent,
    session_repository::SessionRepository,
    upstream_publisher_resolver::UpstreamPublisherResolver,
};
use crate::relay_server::store::RelayStore;

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
        route_registry: Arc<dyn RelayRouteRegistry>,
    ) -> (UnboundedSender<SessionEvent>, Self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<SessionEvent>();
        let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
            repo.clone(),
            sender.clone(),
        ));
        let upstream_publisher_resolver = Arc::new(UpstreamPublisherResolver::new(
            route_registry.clone(),
            inter_relay_connection_manager.clone(),
        ));
        let ingress = IngressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.object_notify_producer_map.clone(),
        );
        let egress = EgressCoordinator::new(
            repo.clone(),
            store.cache_store.clone(),
            store.object_notify_producer_map.clone(),
        );
        let manager = EventHandler::run(
            repo,
            receiver,
            ingress.sender(),
            egress.sender(),
            route_registry,
            inter_relay_connection_manager,
            upstream_publisher_resolver,
            store.cache_store.clone(),
        );
        let evict_job = spawn_cache_eviction_job(store.cache_store.clone());
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
