use std::{sync::Arc, time::Duration};

use moqt::server::ServerConfig;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    RelayConfig,
    modules::{
        auth::session_authenticator::SessionAuthenticator,
        cascading::route_registry::{
            NoopRelayRouteRegistry, RedisRelayRouteRegistry, RelayInfo, RelayRouteRegistry,
        },
        domain::session_peer::SessionPeer,
        observability::stats_publish_task::{StatsPublishConfig, StatsPublishTask},
        session::{session_event::SessionEvent, session_repository::SessionRepository},
    },
    relay_server::{
        drain::drain_client_sessions,
        runtime::RelayRuntime,
        session_handler::{SessionHandler, SessionIntake},
    },
};

pub(crate) struct RelayServerDeps {
    pub(crate) repo: SessionRepository,
    pub(crate) route_registry: Arc<dyn RelayRouteRegistry>,
    pub(crate) authenticator: SessionAuthenticator,
    pub(crate) relay_token: String,
}

pub struct RelayServer {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    sender: UnboundedSender<SessionEvent>,
    authenticator: Arc<SessionAuthenticator>,
    relay_token: String,
    route_registry: Arc<dyn RelayRouteRegistry>,
    runtime: RelayRuntime,
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
        };
        let repo = SessionRepository::new();
        let route_registry: Arc<dyn RelayRouteRegistry> = if let Some(redis_url) = config.redis_url
        {
            RedisRelayRouteRegistry::connect(
                &redis_url,
                relay,
                config.public_host,
                repo.client_session_count(),
            )
            .await?
        } else {
            Arc::new(NoopRelayRouteRegistry)
        };
        let authenticator = SessionAuthenticator::from_config(&config.auth)?;
        Ok(Self::new_with_deps(
            key_path,
            cert_path,
            RelayServerDeps {
                repo,
                route_registry,
                authenticator,
                relay_token: config.auth.relay_token,
            },
        ))
    }

    pub(crate) fn new_with_deps(key_path: &str, cert_path: &str, deps: RelayServerDeps) -> Self {
        let RelayServerDeps {
            repo,
            route_registry,
            authenticator,
            relay_token,
        } = deps;
        let repo = Arc::new(tokio::sync::Mutex::new(repo));
        let (sender, runtime) = RelayRuntime::new(
            repo.clone(),
            route_registry.clone(),
            relay_token.clone(),
            authenticator.verifier.clone(),
        );

        Self {
            repo,
            sender,
            authenticator: Arc::new(authenticator),
            relay_token,
            route_registry,
            runtime,
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
        self.spawn_transport::<T>(port, SessionPeer::Relay)
    }

    /// Sends GOAWAY to every client session and resolves once they have all
    /// left or `timeout` has passed, closing the rest with GOAWAY_TIMEOUT.
    /// Stop the client transport's accept loop first, so that no session
    /// arrives after the GOAWAYs were sent.
    pub async fn drain(&self, timeout: Duration) {
        drain_client_sessions(&self.repo, self.route_registry.as_ref(), timeout).await;
    }

    pub fn spawn_stats_publisher(&self, relay_id: String, inner_port: u16) -> StatsPublishTask {
        StatsPublishTask::run(
            self.runtime.stats_collector(relay_id),
            StatsPublishConfig {
                inner_port,
                relay_token: self.relay_token.clone(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moqt::{SessionEvent, TerminationErrorCode};

    use crate::modules::auth::{
        test_support::spawn_relay_and_connect_client, verified_token::VerifiedToken,
    };

    const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

    async fn receive_go_away(client: &moqt::Session) -> String {
        tokio::time::timeout(EVENT_TIMEOUT, async {
            loop {
                if let SessionEvent::GoAway(go_away) = client.receive_event().await.unwrap() {
                    return go_away.new_session_uri().to_string();
                }
            }
        })
        .await
        .expect("GOAWAY must reach the client")
    }

    #[tokio::test]
    async fn drain_sends_go_away_and_closes_a_lingering_client_with_goaway_timeout() {
        // Arrange
        let (mut relay, client) = spawn_relay_and_connect_client(VerifiedToken::anonymous()).await;
        relay.handler.stop_accepting();

        // Act
        relay.server.drain(Duration::from_millis(200)).await;

        // Assert: no route registry, so the client is told to reuse its URI
        assert_eq!(receive_go_away(&client).await, "");
        let close = tokio::time::timeout(EVENT_TIMEOUT, client.closed())
            .await
            .expect("the relay must close the lingering session");
        assert_eq!(close.code, Some(0x10));
    }

    #[tokio::test]
    async fn drain_returns_when_the_client_closes_after_go_away() {
        // Arrange
        let (mut relay, client) = spawn_relay_and_connect_client(VerifiedToken::anonymous()).await;
        relay.handler.stop_accepting();
        let client_leaves = tokio::spawn(async move {
            receive_go_away(&client).await;
            client.close_with_error(TerminationErrorCode::NoError, "migrated");
            client
        });

        // Act
        let drained =
            tokio::time::timeout(EVENT_TIMEOUT, relay.server.drain(Duration::from_secs(30))).await;

        // Assert
        assert!(
            drained.is_ok(),
            "drain must return once the client has left"
        );
        let client = client_leaves.await.unwrap();
        drop(client);
        let idle = tokio::time::timeout(EVENT_TIMEOUT, relay.handler.wait_idle()).await;
        assert!(
            idle.is_ok(),
            "the client endpoint must become idle after the drain"
        );
    }
}
