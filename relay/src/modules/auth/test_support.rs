use std::{net::UdpSocket, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use moqt::{
    ClientConfig, Endpoint, QUIC, Session,
    wire::{AuthorizationToken, ClientSetup, SetupParameter},
};
use rcgen::{CertifiedKey, generate_simple_self_signed};

use crate::{
    RelayServer,
    modules::{
        auth::{
            session_authenticator::SessionAuthenticator,
            token_verifier::{TokenVerifier, VerifyError},
            verified_token::VerifiedToken,
        },
        route_registry::NoopRelayRouteRegistry,
        session_handler::SessionHandler,
    },
    relay_server::server::RelayServerDeps,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn client_setup(authorization_token: Vec<AuthorizationToken>) -> ClientSetup {
    ClientSetup::new(
        vec![moqt::wire::MOQ_TRANSPORT_VERSION],
        SetupParameter {
            path: None,
            max_request_id: 1,
            authorization_token,
            max_auth_token_cache_size: None,
            authority: None,
            moq_implementation: None,
        },
    )
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
    _server: RelayServer,
    _handler: SessionHandler,
}

fn free_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

fn write_self_signed_cert(dir: &Path) -> (String, String) {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
    (
        key_path.to_string_lossy().into_owned(),
        cert_path.to_string_lossy().into_owned(),
    )
}

/// Starts a relay whose client endpoint verifies every token as `token`.
pub(crate) async fn spawn_relay_with_verifier(token: VerifiedToken) -> RunningRelay {
    let port = free_udp_port();
    let cert_dir = std::env::temp_dir().join(format!("relay-auth-test-{port}"));
    let (key_path, cert_path) = write_self_signed_cert(&cert_dir);
    let server = RelayServer::new_with_deps(
        &key_path,
        &cert_path,
        RelayServerDeps {
            route_registry: Arc::new(NoopRelayRouteRegistry),
            authenticator: SessionAuthenticator::Enabled {
                verifier: Arc::new(StubVerifier(StubOutcome::Verified(token))),
            },
            relay_token: None,
        },
    );
    let handler = server.spawn_client_transport::<QUIC>(port);
    RunningRelay {
        port,
        _server: server,
        _handler: handler,
    }
}

pub(crate) async fn connect_client_with_token(port: u16, token: &str) -> Session<QUIC> {
    let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: Some(token.to_string()),
    })
    .unwrap();
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        endpoint
            .connect(&format!("moqt://127.0.0.1:{port}"))
            .await?
            .await
    })
    .await
    .expect("handshake timed out")
    .expect("handshake failed")
}
