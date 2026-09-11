use std::sync::Arc;

use moqt::ServerConfig;
use tokio::sync::mpsc::UnboundedSender;

use crate::relay_server::{
    runtime::{CascadingDeps, RelayRuntime},
    store::RelayStore,
};
use crate::{
    RelayConfig,
    modules::{
        auth::session_authenticator::SessionAuthenticator,
        route_registry::{
            NoopRelayRouteRegistry, RedisRelayRouteRegistry, RelayInfo, RelayRouteRegistry,
            RouteStatus,
        },
        session_event::SessionEvent,
        session_handler::{SessionHandler, SessionIntake},
        session_repository::{SessionPeer, SessionRepository},
    },
};

pub(crate) struct RelayServerDeps {
    pub(crate) route_registry: Arc<dyn RelayRouteRegistry>,
    pub(crate) authenticator: SessionAuthenticator,
    pub(crate) relay_token: String,
}

pub struct RelayServer {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    sender: UnboundedSender<SessionEvent>,
    authenticator: Arc<SessionAuthenticator>,
    _store: Arc<RelayStore>,
    _runtime: RelayRuntime,
    key_path: String,
    cert_path: String,
}

impl RelayServer {
    pub async fn new_with_config(
        key_path: &str,
        cert_path: &str,
        config: RelayConfig,
    ) -> anyhow::Result<Self> {
        let relay = RelayInfo {
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
        let authenticator = SessionAuthenticator::from_config(&config.auth)?;
        Ok(Self::new_with_deps(
            key_path,
            cert_path,
            RelayServerDeps {
                route_registry,
                authenticator,
                relay_token: config.auth.relay_token,
            },
        ))
    }

    pub(crate) fn new_with_deps(key_path: &str, cert_path: &str, deps: RelayServerDeps) -> Self {
        let RelayServerDeps {
            route_registry,
            authenticator,
            relay_token,
        } = deps;
        let repo = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let store = RelayStore::new();
        let (sender, runtime) = RelayRuntime::new(
            repo.clone(),
            &store,
            CascadingDeps {
                route_registry,
                relay_token,
            },
        );

        Self {
            repo,
            sender,
            authenticator: Arc::new(authenticator),
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
            SessionIntake {
                repo: self.repo.clone(),
                relay_session_event_sender: self.sender.clone(),
                accepted_peer,
                authenticator: self.authenticator.clone(),
            },
        )
    }

    pub fn spawn_client_transport<T: moqt::TransportProtocol>(&self, port: u16) -> SessionHandler {
        self.spawn_transport::<T>(port, SessionPeer::Client)
    }

    pub fn spawn_inner_transport<T: moqt::TransportProtocol>(&self, port: u16) -> SessionHandler {
        self.spawn_transport::<T>(port, SessionPeer::Relay { relay_id: None })
    }
}
