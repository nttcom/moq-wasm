use std::{collections::HashMap, sync::Arc, time::Duration};

use dashmap::DashMap;
use moqt::CertificateVerification;
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

#[derive(Clone)]
pub(crate) struct InterRelayClientConfig {
    /// Sent in CLIENT_SETUP; it grants full access, so it goes only to a relay
    /// whose certificate passes `certificate_verification`.
    pub(crate) relay_token: String,
    pub(crate) certificate_verification: CertificateVerification,
    /// The drafts offered to another relay's inner transport.
    pub(crate) versions: Vec<moqt::Version>,
}

pub(crate) struct InterRelayConnectionManager {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    client_config: InterRelayClientConfig,
    sessions: DashMap<String, SessionId>,
}

impl InterRelayConnectionManager {
    pub(crate) fn new(
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        client_config: InterRelayClientConfig,
    ) -> Self {
        Self {
            repo,
            client_config,
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
                certificate_verification: self.client_config.certificate_verification.clone(),
                authorization_token: Some(self.client_config.relay_token.clone().into()),
                follow_go_away: false,
                versions: self.client_config.versions.clone(),
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
    use std::path::PathBuf;

    use moqt::server::write_self_signed_cert;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::modules::{
        auth::test_support::{
            RunningRelay, app_token, relay_server_offering, spawn_relay_with_verifier,
        },
        cascading::route_registry::NoopRelayRouteRegistry,
        test_support::{
            closed_spans::ClosedSpans, mock_session::detached_session_repository,
            relay_harness::fixtures::disk_cache::TempDir,
        },
    };

    fn manager_trusting(trusted_cert_path: PathBuf) -> InterRelayConnectionManager {
        manager_offering(trusted_cert_path, moqt::DEFAULT_VERSIONS)
    }

    fn manager_offering(
        trusted_cert_path: PathBuf,
        versions: &[moqt::Version],
    ) -> InterRelayConnectionManager {
        InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
            InterRelayClientConfig {
                relay_token: "relay-token".to_string(),
                certificate_verification: CertificateVerification::TrustedCertificates(
                    trusted_cert_path,
                ),
                versions: versions.to_vec(),
            },
        )
    }

    fn relay_info(relay: &RunningRelay) -> RelayInfo {
        RelayInfo {
            relay_id: "relay-1".to_string(),
            host: "127.0.0.1".to_string(),
            port: relay.port,
        }
    }

    #[tokio::test]
    async fn the_request_that_dials_a_relay_closes_while_the_session_lives() {
        // Arrange
        let closed_spans = ClosedSpans::default();
        let _default = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(closed_spans.clone()),
        );
        let relay = spawn_relay_with_verifier(app_token(Some(""), Some(""))).await;
        let manager = manager_trusting(relay.cert_path.clone());
        let request_span = tracing::info_span!("relay.publish_namespace");

        // Act
        manager
            .get_or_connect(&relay_info(&relay))
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

    #[tokio::test]
    async fn a_relay_whose_certificate_is_not_trusted_is_not_dialed() {
        // Arrange: a callsite first reached on a thread without a subscriber can
        // be cached as disabled for every thread, which would disable
        // `relay.inter_relay.session` in a test that records spans while dialing.
        let _default = tracing::subscriber::set_default(tracing_subscriber::registry());
        let relay = spawn_relay_with_verifier(app_token(Some(""), Some(""))).await;
        let other_cert_dir = TempDir::new("inter-relay-untrusted");
        let other_cert =
            write_self_signed_cert(other_cert_dir.path(), &["localhost", "127.0.0.1"]).unwrap();
        let manager = manager_trusting(PathBuf::from(other_cert.cert_path));

        // Act
        let result = manager.get_or_connect(&relay_info(&relay)).await;

        // Assert
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("invalid peer certificate"), "{error}");
        assert!(manager.dialed_relay_ids().is_empty());
    }

    #[tokio::test]
    async fn a_relay_dials_another_in_the_drafts_it_is_configured_with() {
        // Arrange: the other relay accepts draft-18 only
        let other = relay_server_offering(
            VerifiedToken::full_access(),
            Arc::new(NoopRelayRouteRegistry),
            &[moqt::Version::Draft18],
        );
        let inner = other.server.spawn_inner_transport::<moqt::QUIC>(0).unwrap();
        let manager = manager_offering(
            other.cert_path.clone(),
            &[moqt::Version::Draft18, moqt::Version::Draft14],
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
