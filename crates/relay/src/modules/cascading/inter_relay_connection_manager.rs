use std::{collections::HashMap, sync::Arc, time::Duration};

use dashmap::DashMap;

use crate::{
    logging::relay_hostname,
    modules::{
        auth::verified_token::VerifiedToken,
        cascading::route_registry::RelayInfo,
        domain::{
            session_id::{SessionId, generate_session_id},
            session_peer::SessionPeer,
        },
        session::{
            session_event::SessionEvent,
            session_repository::{NewSession, SessionRepository},
        },
    },
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct InterRelayConnectionManager {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    relay_token: String,
    /// The drafts offered to another relay's inner transport.
    versions: Vec<moqt::Version>,
    sessions: DashMap<String, SessionId>,
}

impl InterRelayConnectionManager {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        relay_token: String,
        versions: Vec<moqt::Version>,
    ) -> Self {
        Self {
            repo,
            session_event_sender,
            relay_token,
            versions,
            sessions: DashMap::new(),
        }
    }

    pub(crate) fn dialed_relay_ids(&self) -> HashMap<SessionId, String> {
        self.sessions
            .iter()
            .map(|entry| (*entry.value(), entry.key().clone()))
            .collect()
    }

    pub(crate) async fn get_or_connect(&self, relay: &RelayInfo) -> anyhow::Result<SessionId> {
        if let Some(session_id) = self.sessions.get(&relay.relay_id)
            && self.repo.lock().await.has_session(*session_id)
        {
            return Ok(*session_id);
        }

        let session_id = generate_session_id();
        let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client(&moqt::ClientConfig {
            port: 0,
            verify_certificate: false,
            authorization_token: Some(self.relay_token.clone()),
            follow_go_away: false,
            versions: self.versions.clone(),
        })?;
        let session = tokio::time::timeout(CONNECT_TIMEOUT, async {
            endpoint
                .connect(&format!("moqt://{}:{}", relay.host, relay.port))
                .await?
                .await
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out connecting relay {}", relay.relay_id))??;
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
            .add(
                NewSession {
                    session_id,
                    session: Box::new(session),
                    session_span,
                    peer: SessionPeer::Relay,
                    verified_token: VerifiedToken::full_access(),
                },
                self.session_event_sender.clone(),
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
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::InterRelayConnectionManager;
    use crate::modules::{
        auth::{test_support::relay_server_offering, verified_token::VerifiedToken},
        cascading::route_registry::RelayInfo,
        session::session_repository::SessionRepository,
    };

    #[tokio::test]
    async fn a_relay_dials_another_in_the_drafts_it_is_configured_with() {
        // Arrange: the other relay accepts draft-18 only
        let (other, _cert_dir) =
            relay_server_offering(VerifiedToken::full_access(), &[moqt::Version::Draft18]);
        let inner = other.spawn_inner_transport::<moqt::QUIC>(0).unwrap();
        let (session_event_sender, _session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let manager = InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(SessionRepository::new())),
            session_event_sender,
            "relay-token".to_string(),
            vec![moqt::Version::Draft18, moqt::Version::Draft14],
        );

        // Act
        let dialed = tokio::time::timeout(
            Duration::from_secs(5),
            manager.get_or_connect(&RelayInfo {
                relay_id: "relay-b".to_string(),
                host: "127.0.0.1".to_string(),
                port: inner.local_port,
            }),
        )
        .await
        .expect("the dial should end");

        // Assert
        assert!(dialed.is_ok(), "{:?}", dialed.err());
    }
}
