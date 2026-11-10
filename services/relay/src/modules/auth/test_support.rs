use moqt::RequestErrorCode;
use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use moqt::{
    CertificateVerification, ClientConfig, Endpoint, QUIC, RequestFailure, Session,
    server::{CertificateFiles, write_self_signed_cert},
};

use crate::{
    CacheConfig, RelayServer,
    config::DEFAULT_FETCH_FILL_TIMEOUT,
    modules::{
        auth::{
            session_authenticator::SessionAuthenticator,
            token_verifier::{TokenVerifier, VerifyError},
            verified_token::{VerifiedToken, parse_namespace_path},
        },
        cascading::route_registry::{NoopRelayRouteRegistry, RelayRouteRegistry},
        session::session_repository::SessionRepository,
        test_support::{
            directory_fixtures::unused_inter_relay_client_config,
            relay_harness::fixtures::disk_cache::TempDir,
        },
    },
    relay_server::{
        runtime::DataPlaneConfig, server::RelayServerDeps, session_handler::SessionHandler,
    },
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn app_token(publish: Option<&str>, subscribe: Option<&str>) -> VerifiedToken {
    VerifiedToken {
        app_id: "APP".to_string(),
        publish: publish.map(parse_namespace_path),
        subscribe: subscribe.map(parse_namespace_path),
        is_relay: false,
        expires_at: None,
        relay_id: None,
    }
}

pub(crate) fn relay_token() -> VerifiedToken {
    VerifiedToken {
        is_relay: true,
        ..app_token(Some(""), Some(""))
    }
}

pub(crate) fn request_error_code(error: impl Into<anyhow::Error>) -> RequestErrorCode {
    let error = error.into();
    match error.downcast_ref::<RequestFailure>() {
        Some(RequestFailure::Rejected { code, .. }) => *code,
        _ => panic!("expected a request rejection, got {error:?}"),
    }
}

pub(crate) enum StubOutcome {
    Verified(VerifiedToken),
    Unauthorized,
    Unavailable,
}

pub(crate) struct StubVerifier(pub(crate) StubOutcome);

#[async_trait]
impl TokenVerifier for StubVerifier {
    async fn verify(&self, _token: &str) -> Result<VerifiedToken, VerifyError> {
        match &self.0 {
            StubOutcome::Verified(token) => Ok(token.clone()),
            StubOutcome::Unauthorized => {
                Err(VerifyError::Unauthorized("invalid_signature".to_string()))
            }
            StubOutcome::Unavailable => Err(VerifyError::Unavailable(anyhow::anyhow!(
                "connection refused"
            ))),
        }
    }
}

pub(crate) struct RunningRelay {
    pub(crate) port: u16,
    pub(crate) server: RelayServer,
    pub(crate) handler: SessionHandler,
    /// Self-signed for `localhost` and `127.0.0.1`.
    pub(crate) cert_path: PathBuf,
    _cert_dir: TempDir,
}

pub(crate) async fn spawn_relay_with_verifier(token: VerifiedToken) -> RunningRelay {
    spawn_relay(token, Arc::new(NoopRelayRouteRegistry)).await
}

/// The tests write and read draft-14 messages.
pub(crate) const TEST_VERSIONS: &[moqt::Version] = &[moqt::Version::Draft14];

pub(crate) async fn spawn_relay(
    token: VerifiedToken,
    route_registry: Arc<dyn RelayRouteRegistry>,
) -> RunningRelay {
    spawn_relay_offering(token, route_registry, TEST_VERSIONS)
}

/// A relay that accepts only draft-18 sessions; connect to it with
/// `connect_draft18_client`.
pub(crate) fn spawn_draft18_relay(token: VerifiedToken) -> RunningRelay {
    spawn_relay_offering(
        token,
        Arc::new(NoopRelayRouteRegistry),
        &[moqt::Version::Draft18],
    )
}

fn spawn_relay_offering(
    token: VerifiedToken,
    route_registry: Arc<dyn RelayRouteRegistry>,
    versions: &[moqt::Version],
) -> RunningRelay {
    let OfferingRelay {
        server,
        cert_path,
        cert_dir,
    } = relay_server_offering(token, route_registry, versions);
    let handler = server.spawn_client_transport::<QUIC>(0).unwrap();
    RunningRelay {
        port: handler.local_port,
        server,
        handler,
        cert_path,
        _cert_dir: cert_dir,
    }
}

/// A relay with no transport spawned yet; the certificate lives as long as
/// `cert_dir`.
pub(crate) struct OfferingRelay {
    pub(crate) server: RelayServer,
    /// Self-signed for `localhost` and `127.0.0.1`.
    pub(crate) cert_path: PathBuf,
    pub(crate) cert_dir: TempDir,
}

/// A relay accepting `versions` whose verifier takes every token as `token`.
pub(crate) fn relay_server_offering(
    token: VerifiedToken,
    route_registry: Arc<dyn RelayRouteRegistry>,
    versions: &[moqt::Version],
) -> OfferingRelay {
    let cert_dir = TempDir::new("auth-test");
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(cert_dir.path(), &["localhost", "127.0.0.1"]).unwrap();
    let (session_added_sender, session_added_receiver) = tokio::sync::mpsc::unbounded_channel();
    let server = RelayServer::new_with_deps(
        &key_path,
        &cert_path,
        RelayServerDeps {
            repo: SessionRepository::new(session_added_sender),
            session_added_receiver,
            route_registry,
            authenticator: SessionAuthenticator {
                verifier: Arc::new(StubVerifier(StubOutcome::Verified(token))),
            },
            inter_relay_client: unused_inter_relay_client_config(),
            versions: versions.to_vec(),
            data_plane: DataPlaneConfig {
                cache: CacheConfig::default(),
                disk_cache: None,
                fetch_fill_timeout: DEFAULT_FETCH_FILL_TIMEOUT,
            },
        },
    );
    OfferingRelay {
        server,
        cert_path: PathBuf::from(&cert_path),
        cert_dir,
    }
}

pub(crate) fn client_endpoint(authorization_token: Option<String>) -> Endpoint<QUIC> {
    client_endpoint_offering(authorization_token, moqt::DEFAULT_VERSIONS)
}

fn client_endpoint_offering(
    authorization_token: Option<String>,
    versions: &[moqt::Version],
) -> Endpoint<QUIC> {
    Endpoint::<QUIC>::create_client(&ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        authorization_token: authorization_token.map(Into::into),
        follow_go_away: false,
        versions: versions.to_vec(),
        ..ClientConfig::default()
    })
    .unwrap()
}

pub(crate) async fn spawn_relay_and_connect_client(
    token: VerifiedToken,
) -> (RunningRelay, Session) {
    let relay = spawn_relay_with_verifier(token).await;
    let client = connect_client(&relay).await;
    (relay, client)
}

pub(crate) async fn connect_client(relay: &RunningRelay) -> Session {
    connect(relay, client_endpoint(Some("jwt".to_string()))).await
}

pub(crate) async fn connect_draft18_client(relay: &RunningRelay) -> Session {
    let endpoint = client_endpoint_offering(Some("jwt".to_string()), &[moqt::Version::Draft18]);
    connect(relay, endpoint).await
}

async fn connect(relay: &RunningRelay, endpoint: Endpoint<QUIC>) -> Session {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        endpoint
            .connect(&format!("moqt://127.0.0.1:{}", relay.port))
            .await?
            .await
    })
    .await
    .expect("handshake timed out")
    .expect("handshake failed")
}
