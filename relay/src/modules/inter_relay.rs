use std::{net::SocketAddr, sync::Arc};

use dashmap::DashMap;

use crate::modules::{
    route_registry::RelayInfo,
    session_event::SessionEvent,
    session_repository::SessionRepository,
    types::{SessionId, generate_session_id},
};

fn relay_hostname() -> String {
    std::env::var("RELAY_HOSTNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) struct InterRelayConnectionManager {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    sessions: DashMap<String, SessionId>,
}

impl InterRelayConnectionManager {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            repo,
            session_event_sender,
            sessions: DashMap::new(),
        }
    }

    pub(crate) async fn get_or_connect(&self, relay: &RelayInfo) -> anyhow::Result<SessionId> {
        if let Some(session_id) = self.sessions.get(&relay.relay_id)
            && self.repo.lock().await.has_session(*session_id)
        {
            return Ok(*session_id);
        }

        let session_id = generate_session_id();
        let remote_address = self.resolve_remote_address(relay).await?;
        let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client(&moqt::ClientConfig {
            port: 0,
            verify_certificate: false,
        })?;
        let connecting = endpoint.connect(remote_address, &relay.host).await?;
        let session = connecting.await?;
        let relay_hostname = relay_hostname();
        let session_span = tracing::info_span!(
            "relay.inter_relay.session",
            relay_id = %relay.relay_id,
            session_id = session_id,
            session_peer = "relay",
            session_peer_relay_id = %relay.relay_id,
            relay_hostname = %relay_hostname,
            relay_host = %relay.host,
            relay_port = relay.port,
        );

        self.repo
            .lock()
            .await
            .add_relay(
                session_id,
                Box::new(session),
                self.session_event_sender.clone(),
                session_span,
                Some(relay.relay_id.clone()),
            )
            .await;
        self.sessions.insert(relay.relay_id.clone(), session_id);
        tracing::info!(
            relay_id = %relay.relay_id,
            relay_host = %relay.host,
            relay_port = relay.port,
            session_id = session_id,
            "inter-relay QUIC session established"
        );
        Ok(session_id)
    }

    async fn resolve_remote_address(&self, relay: &RelayInfo) -> anyhow::Result<SocketAddr> {
        let address = format!("{}:{}", relay.host, relay.port);
        tokio::net::lookup_host(address)
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("failed to resolve relay {}", relay.relay_id))
    }
}
