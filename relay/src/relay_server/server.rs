use std::sync::Arc;

use moqt::ServerConfig;
use tokio::sync::mpsc::UnboundedSender;

use crate::relay_server::{runtime::RelayRuntime, store::RelayStore};
use crate::{
    RelayConfig,
    modules::{
        route_registry::{
            NoopRelayRouteRegistry, RedisRelayRouteRegistry, RelayDescriptor, RelayRouteRegistry,
            RouteStatus,
        },
        session_event::SessionEvent,
        session_handler::SessionHandler,
        session_repository::{SessionPeer, SessionRepository},
    },
};

pub struct RelayServer {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    sender: UnboundedSender<SessionEvent>,
    _store: Arc<RelayStore>,
    _runtime: RelayRuntime,
    key_path: String,
    cert_path: String,
}

impl RelayServer {
    pub fn new(key_path: &str, cert_path: &str) -> Self {
        let route_registry: Arc<dyn RelayRouteRegistry> = Arc::new(NoopRelayRouteRegistry);
        Self::new_with_route_registry(key_path, cert_path, route_registry)
    }

    pub async fn new_with_config(
        key_path: &str,
        cert_path: &str,
        config: RelayConfig,
    ) -> anyhow::Result<Self> {
        let relay = RelayDescriptor {
            relay_id: config.relay_id,
            host: config.advertise_host,
            port: config.inner_port,
            status: RouteStatus::Active,
        };
        let route_registry: Arc<dyn RelayRouteRegistry> = if let Some(redis_url) = config.redis_url
        {
            RedisRelayRouteRegistry::connect(&redis_url, relay).await?
        } else {
            Arc::new(NoopRelayRouteRegistry)
        };
        Ok(Self::new_with_route_registry(
            key_path,
            cert_path,
            route_registry,
        ))
    }

    fn new_with_route_registry(
        key_path: &str,
        cert_path: &str,
        route_registry: Arc<dyn RelayRouteRegistry>,
    ) -> Self {
        let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let store = RelayStore::new();
        let (sender, runtime) = RelayRuntime::new(repo.clone(), &store, route_registry);

        Self {
            repo,
            sender,
            _store: store,
            _runtime: runtime,
            key_path: key_path.to_string(),
            cert_path: cert_path.to_string(),
        }
    }

    fn spawn_transport<T: moqt::TransportProtocol>(
        &self,
        port: u16,
        accepted_peer: SessionPeer,
    ) -> SessionHandler {
        tracing::info!(port = port, peer = ?accepted_peer, "Spawning transport handler");
        let server_config = ServerConfig {
            port,
            cert_path: self.cert_path.clone(),
            key_path: self.key_path.clone(),
            keep_alive_interval_sec: 15,
        };

        SessionHandler::run::<T>(
            server_config,
            self.repo.clone(),
            self.sender.clone(),
            accepted_peer,
        )
    }

    pub fn spawn_client_transport<T: moqt::TransportProtocol>(&self, port: u16) -> SessionHandler {
        self.spawn_transport::<T>(port, SessionPeer::Client)
    }

    pub fn spawn_inner_transport<T: moqt::TransportProtocol>(&self, port: u16) -> SessionHandler {
        self.spawn_transport::<T>(port, SessionPeer::Relay { relay_id: None })
    }
}
