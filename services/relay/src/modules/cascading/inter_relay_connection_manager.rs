use std::{collections::HashMap, sync::Arc, time::Duration};

use dashmap::DashMap;
use tracing::Instrument;

use crate::{
    logging::relay_hostname,
    modules::{
        auth::verified_token::VerifiedToken,
        cascading::route_registry::RelayInfo,
        domain::{
            session_id::{SessionId, generate_session_id},
            session_peer::SessionPeer,
        },
        session::session_repository::{NewSession, SessionRepository},
    },
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct InterRelayConnectionManager {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    relay_token: String,
    sessions: DashMap<String, SessionId>,
}

impl InterRelayConnectionManager {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_token: String,
    ) -> Self {
        Self {
            repo,
            relay_token,
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
        let relay_hostname = relay_hostname();
        // A session outlives the request that dials it, so its span roots its own trace.
        // moqt creates its session span inside this one.
        let session_span = tracing::info_span!(
            parent: None,
            "relay.inter_relay.session",
            relay_id = %relay.relay_id,
            session_id = session_id,
            session_peer = "relay",
            relay_hostname = %relay_hostname,
            relay_host = %relay.host,
            relay_port = relay.port,
        );
        // quinn spawns the endpoint driver in the span current at creation, which
        // would hold the dialing request's span open for the endpoint's lifetime.
        let endpoint = session_span.in_scope(|| {
            moqt::Endpoint::<moqt::QUIC>::create_client(&moqt::ClientConfig {
                certificate_verification: moqt::CertificateVerification::Disabled,
                authorization_token: Some(self.relay_token.clone().into()),
                follow_go_away: false,
                ..moqt::ClientConfig::default()
            })
        })?;
        let session = tokio::time::timeout(CONNECT_TIMEOUT, async {
            endpoint
                .connect(&format!("moqt://{}:{}", relay.host, relay.port))
                .await?
                .await
        })
        .instrument(session_span.clone())
        .await
        .map_err(|_| anyhow::anyhow!("timed out connecting relay {}", relay.relay_id))??;

        self.repo.lock().await.add(NewSession {
            session_id,
            session: Box::new(session),
            session_span,
            peer: SessionPeer::Relay,
            verified_token: VerifiedToken {
                relay_id: Some(relay.relay_id.clone()),
                ..VerifiedToken::full_access()
            },
        });
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
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::modules::{
        auth::test_support::{app_token, spawn_relay_with_verifier},
        test_support::{closed_spans::ClosedSpans, mock_session::detached_session_repository},
    };

    #[tokio::test]
    async fn the_request_that_dials_a_relay_closes_while_the_session_lives() {
        // Arrange
        let closed_spans = ClosedSpans::default();
        let _default = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(closed_spans.clone()),
        );
        let relay = spawn_relay_with_verifier(app_token(Some(""), Some(""))).await;
        let manager = InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
            "relay-token".to_string(),
        );
        let relay_info = RelayInfo {
            relay_id: "relay-1".to_string(),
            host: "127.0.0.1".to_string(),
            port: relay.port,
        };
        let request_span = tracing::info_span!("relay.publish_namespace");

        // Act
        manager
            .get_or_connect(&relay_info)
            .instrument(request_span.clone())
            .await
            .unwrap();
        drop(request_span);

        // Assert
        assert!(
            closed_spans.contains("relay.publish_namespace"),
            "closed spans: {:?}",
            closed_spans.names()
        );
    }
}
