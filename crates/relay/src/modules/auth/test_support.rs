use moqt::RequestErrorCode;
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use moqt::{
    ClientConfig, Endpoint, QUIC, RequestFailure, Session,
    server::{CertificateFiles, write_self_signed_cert},
};

use crate::{
    RelayServer,
    modules::{
        auth::{
            session_authenticator::SessionAuthenticator,
            token_verifier::{TokenVerifier, VerifyError},
            verified_token::{VerifiedToken, parse_namespace_path},
        },
        cascading::route_registry::NoopRelayRouteRegistry,
        session::session_repository::SessionRepository,
        test_support::relay_harness::fixtures::disk_cache::TempDir,
    },
    relay_server::{server::RelayServerDeps, session_handler::SessionHandler},
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn app_token(publish: Option<&str>, subscribe: Option<&str>) -> VerifiedToken {
    VerifiedToken {
        app_id: "APP".to_string(),
        publish: publish.map(parse_namespace_path),
        subscribe: subscribe.map(parse_namespace_path),
        is_relay: false,
        expires_at: None,
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
    _cert_dir: TempDir,
}

pub(crate) async fn spawn_relay_with_verifier(token: VerifiedToken) -> RunningRelay {
    let cert_dir = TempDir::new("auth-test");
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(cert_dir.path(), &["localhost", "127.0.0.1"]).unwrap();
    let server = RelayServer::new_with_deps(
        &key_path,
        &cert_path,
        RelayServerDeps {
            repo: SessionRepository::new(),
            route_registry: Arc::new(NoopRelayRouteRegistry),
            authenticator: SessionAuthenticator {
                verifier: Arc::new(StubVerifier(StubOutcome::Verified(token))),
            },
            relay_token: "unused-relay-token".to_string(),
            disk_cache: None,
        },
    );
    let handler = server.spawn_client_transport::<QUIC>(0).unwrap();
    RunningRelay {
        port: handler.local_port,
        server,
        handler,
        _cert_dir: cert_dir,
    }
}

pub(crate) fn client_endpoint(authorization_token: Option<String>) -> Endpoint<QUIC> {
    Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token,
    })
    .unwrap()
}

pub(crate) async fn spawn_relay_and_connect_client(
    token: VerifiedToken,
) -> (RunningRelay, Session) {
    let relay = spawn_relay_with_verifier(token).await;
    let endpoint = client_endpoint(Some("jwt".to_string()));
    let client = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        endpoint
            .connect(&format!("moqt://127.0.0.1:{}", relay.port))
            .await?
            .await
    })
    .await
    .expect("handshake timed out")
    .expect("handshake failed");
    (relay, client)
}
