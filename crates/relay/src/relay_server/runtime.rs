use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::{
    config::DiskCacheConfig,
    modules::{
        auth::token_verifier::TokenVerifier,
        cascading::{
            inter_relay_connection_manager::InterRelayConnectionManager,
            route_registry::RelayRouteRegistry, watched_namespace_job::WatchedNamespaceJob,
        },
        control_plane::{
            control_message_forwarder::ControlMessageForwarder,
            event_handler::{EventHandler, WorkerDeps},
            upstream_creation_serializer::UpstreamCreationSerializer,
            upstream_publisher_resolver::UpstreamPublisherResolver,
        },
        data_plane::{
            cache::{
                disk_cache::DiskCacheTask, eviction_job::spawn_cache_eviction_job,
                store::TrackCacheStore,
            },
            egress::coordinator::EgressCoordinator,
            ingress::ingress_coordinator::IngressCoordinator,
        },
        domain::pub_sub_directory::InMemoryLocalPubSubDirectory,
        observability::{
            stats_collector::{StatsCollector, StatsSources},
            stats_publish_task::unix_millis_now,
        },
        session::{session_event::SessionEvent, session_repository::SessionRepository},
    },
};

pub(crate) struct RelayRuntime {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    local_pub_sub_directory: Arc<InMemoryLocalPubSubDirectory>,
    cache_store: Arc<TrackCacheStore>,
    inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    _ingress: IngressCoordinator,
    _egress: EgressCoordinator,
    _manager: EventHandler,
    _evict_job: tokio::task::JoinHandle<()>,
    _watched_namespace_job: WatchedNamespaceJob,
    _disk_cache: Option<DiskCacheTask>,
}

impl RelayRuntime {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        route_registry: Arc<dyn RelayRouteRegistry>,
        relay_token: String,
        token_verifier: Arc<dyn TokenVerifier>,
        disk_cache: Option<DiskCacheConfig>,
    ) -> (UnboundedSender<SessionEvent>, Self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<SessionEvent>();
        let disk_cache = disk_cache.map(DiskCacheTask::run);
        let cache_store = Arc::new(TrackCacheStore::with_disk(
            disk_cache.as_ref().map(DiskCacheTask::handle),
        ));
        let inter_relay_connection_manager = Arc::new(InterRelayConnectionManager::new(
            repo.clone(),
            sender.clone(),
            relay_token,
        ));
        let upstream_publisher_resolver = Arc::new(UpstreamPublisherResolver::new(
            route_registry.clone(),
            inter_relay_connection_manager.clone(),
        ));
        let local_pub_sub_directory = Arc::new(InMemoryLocalPubSubDirectory::new());
        let watched_namespace_job = WatchedNamespaceJob::run(
            upstream_publisher_resolver.watched_namespace_routes.clone(),
            local_pub_sub_directory.clone(),
        );
        let ingress = IngressCoordinator::new(repo.clone(), cache_store.clone(), sender.clone());
        let egress = EgressCoordinator::new(repo.clone(), cache_store.clone(), sender.clone());
        let manager = EventHandler::run(
            receiver,
            WorkerDeps {
                control_message_forwarder: ControlMessageForwarder {
                    repository: repo.clone(),
                },
                repo: repo.clone(),
                relay_event_sender: sender.clone(),
                local_pub_sub_directory: local_pub_sub_directory.clone(),
                ingress_sender: ingress.sender(),
                egress_sender: egress.sender(),
                route_registry,
                inter_relay_connection_manager: inter_relay_connection_manager.clone(),
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
                repo,
                local_pub_sub_directory,
                cache_store,
                inter_relay_connection_manager,
                _ingress: ingress,
                _egress: egress,
                _manager: manager,
                _evict_job: evict_job,
                _watched_namespace_job: watched_namespace_job,
                _disk_cache: disk_cache,
            },
        )
    }

    pub(crate) fn begin_draining(&self) {
        self.local_pub_sub_directory
            .begin_draining(unix_millis_now());
    }

    pub(crate) fn stats_collector(&self, relay_id: String) -> StatsCollector {
        StatsCollector::new(
            relay_id,
            StatsSources {
                repo: self.repo.clone(),
                directory: self.local_pub_sub_directory.clone(),
                cache_store: self.cache_store.clone(),
                inter_relay_connection_manager: self.inter_relay_connection_manager.clone(),
            },
        )
    }
}
