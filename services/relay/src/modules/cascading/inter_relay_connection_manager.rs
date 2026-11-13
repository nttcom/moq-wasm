use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use moqt::CertificateVerification;
use tracing::Instrument;

use crate::{
    logging::relay_hostname,
    modules::{
        auth::verified_token::VerifiedToken,
        cascading::{
            relay_namespace_subscriptions::RelayNamespaceSubscriptions, route_registry::RelayInfo,
        },
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

#[derive(Default)]
struct RelayDial {
    /// Held while the relay is dialed, so the requests that need it at once
    /// share one session: a track's upstream SUBSCRIBE is claimed per session.
    lock: tokio::sync::Mutex<()>,
    /// A request that waited for a dial that failed fails with it instead of
    /// dialing again a relay that does not answer.
    failures: AtomicU64,
}

pub(crate) struct InterRelayConnectionManager {
    repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    client_config: InterRelayClientConfig,
    sessions: DashMap<String, SessionId>,
    dials: DashMap<String, Arc<RelayDial>>,
    pub(crate) namespace_subscriptions: RelayNamespaceSubscriptions,
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
            dials: DashMap::new(),
            namespace_subscriptions: RelayNamespaceSubscriptions::default(),
        }
    }

    pub(crate) fn dialed_relay_ids(&self) -> HashMap<SessionId, String> {
        self.sessions
            .iter()
            .map(|entry| (*entry.value(), entry.key().clone()))
            .collect()
    }

    /// Makes `get_or_connect` return the session for the relay, as if it
    /// had dialed it.
    #[cfg(test)]
    pub(crate) fn record_dialed(&self, relay_id: &str, session_id: SessionId) {
        self.sessions.insert(relay_id.to_string(), session_id);
    }

    pub(crate) async fn get_or_connect(&self, relay: &RelayInfo) -> anyhow::Result<SessionId> {
        let dial = self
            .dials
            .entry(relay.relay_id.clone())
            .or_default()
            .clone();
        let failures_before = dial.failures.load(Ordering::Relaxed);
        let _dialing = dial.lock.lock().await;
        let dialed = self.sessions.get(&relay.relay_id).map(|entry| *entry);
        if let Some(session_id) = dialed
            && self.repo.lock().await.has_session(session_id)
        {
            return Ok(session_id);
        }
        if dial.failures.load(Ordering::Relaxed) != failures_before {
            anyhow::bail!(
                "the dial of relay {} this request waited for failed",
                relay.relay_id
            );
        }
        self.dial(relay).await.inspect_err(|_| {
            dial.failures.fetch_add(1, Ordering::Relaxed);
        })
    }

    async fn dial(&self, relay: &RelayInfo) -> anyhow::Result<SessionId> {
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
            RunningRelay, TEST_VERSIONS, app_token, relay_server_offering,
            spawn_relay_with_verifier,
        },
        cascading::route_registry::NoopRelayRouteRegistry,
        test_support::{
            closed_spans::ClosedSpans, mock_session::detached_session_repository,
            relay_harness::fixtures::disk_cache::TempDir,
        },
    };

    fn manager_trusting(trusted_cert_path: PathBuf) -> InterRelayConnectionManager {
        manager_offering(trusted_cert_path, TEST_VERSIONS)
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
        let _default = telemetry::testing::set_default(
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
    async fn requests_that_need_the_same_relay_at_once_share_one_session() {
        // Arrange
        let relay = spawn_relay_with_verifier(app_token(Some(""), Some(""))).await;
        let manager = manager_trusting(relay.cert_path.clone());
        let relay_info = relay_info(&relay);

        // Act
        let (first, second) = tokio::join!(
            manager.get_or_connect(&relay_info),
            manager.get_or_connect(&relay_info)
        );

        // Assert
        assert_eq!(first.unwrap(), second.unwrap());
    }

    #[tokio::test]
    async fn a_relay_whose_certificate_is_not_trusted_is_not_dialed() {
        // Arrange
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
    async fn a_request_that_waited_for_a_failed_dial_does_not_dial_again() {
        // Arrange: a relay whose certificate the manager does not trust
        let relay = spawn_relay_with_verifier(app_token(Some(""), Some(""))).await;
        let other_cert_dir = TempDir::new("inter-relay-shared-failure");
        let other_cert =
            write_self_signed_cert(other_cert_dir.path(), &["localhost", "127.0.0.1"]).unwrap();
        let manager = manager_trusting(PathBuf::from(other_cert.cert_path));
        let relay_info = relay_info(&relay);

        // Act
        let (dialing, waiting) = tokio::join!(
            manager.get_or_connect(&relay_info),
            manager.get_or_connect(&relay_info)
        );

        // Assert
        let dialing = format!("{:#}", dialing.unwrap_err());
        let waiting = format!("{:#}", waiting.unwrap_err());
        assert!(dialing.contains("invalid peer certificate"), "{dialing}");
        assert!(waiting.contains("waited for failed"), "{waiting}");
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
