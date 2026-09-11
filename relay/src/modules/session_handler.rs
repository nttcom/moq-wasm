use std::sync::Arc;

use moqt::{Accepting, Endpoint, TransportProtocol};
use tracing::{Instrument, Span};

use crate::modules::{
    auth::session_authenticator::SessionAuthenticator,
    session_event::SessionEvent,
    session_repository::{NewSession, SessionPeer, SessionRepository},
    types::{SessionId, generate_session_id},
};

fn relay_hostname() -> String {
    std::env::var("RELAY_HOSTNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[derive(Clone)]
pub(crate) struct SessionIntake {
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    pub(crate) accepted_peer: SessionPeer,
    pub(crate) authenticator: Arc<SessionAuthenticator>,
}

impl SessionIntake {
    async fn establish<T: TransportProtocol>(
        &self,
        session_id: SessionId,
        connecting: Accepting<T>,
        session_span: Span,
    ) {
        let handshake = match connecting.await {
            Ok(handshake) => handshake,
            Err(error) => {
                tracing::warn!(%error, "failed to establish session");
                return;
            }
        };
        let verified_token = match self
            .authenticator
            .authenticate(handshake.client_setup(), &self.accepted_peer)
            .await
        {
            Ok(verified_token) => verified_token,
            Err(rejected) => {
                tracing::warn!(
                    code = ?rejected.code,
                    reason = %rejected.reason,
                    "session rejected at CLIENT_SETUP"
                );
                handshake.reject(rejected.code, &rejected.reason).await;
                return;
            }
        };
        let session = match handshake.accept().await {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(%error, "failed to establish session");
                return;
            }
        };
        let session_add_span = tracing::info_span!(
            parent: &session_span,
            "relay.session_repository.add",
            session_id = session_id
        );
        async {
            tracing::info!("Session accepted");
            self.repo
                .lock()
                .await
                .add(
                    NewSession {
                        session_id,
                        session: Box::new(session),
                        session_span: session_span.clone(),
                        peer: self.accepted_peer.clone(),
                        verified_token,
                    },
                    self.relay_session_event_sender.clone(),
                )
                .await;
        }
        .instrument(session_add_span)
        .await;
    }
}

pub struct SessionHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionHandler {
    pub(crate) fn run<T: TransportProtocol>(
        config: moqt::ServerConfig,
        intake: SessionIntake,
    ) -> Self {
        let endpoint = Endpoint::<T>::create_server(&config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        let join_handle = Self::create_joinhandle::<T>(endpoint, intake);
        Self { join_handle }
    }

    fn create_joinhandle<T: TransportProtocol>(
        mut endpoint: Endpoint<T>,
        intake: SessionIntake,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                let relay_hostname = relay_hostname();
                loop {
                    let session_id = generate_session_id();
                    let session_peer = intake.accepted_peer.kind();
                    let session_peer_relay_id =
                        intake.accepted_peer.relay_id().unwrap_or("unknown");
                    let session_span = tracing::info_span!(
                        parent: None,
                        "relay.session",
                        session_id = session_id,
                        session_peer = session_peer,
                        session_peer_relay_id = session_peer_relay_id,
                        relay_hostname = %relay_hostname,
                    );
                    let connecting = async {
                        endpoint.accept().await.inspect_err(|error| {
                            if Self::is_endpoint_closing(error) {
                                tracing::info!(%error, "transport endpoint closed");
                            } else {
                                tracing::warn!(%error, "failed to accept transport connection");
                            }
                        })
                    }
                    .instrument(session_span.clone())
                    .await;
                    let connecting = match connecting {
                        Ok(connecting) => connecting,
                        Err(error) => {
                            if Self::is_endpoint_closing(&error) {
                                break;
                            }
                            continue;
                        }
                    };

                    // Spawn per connection so a slow ClientSetup cannot block the accept loop.
                    let intake = intake.clone();
                    tokio::spawn(async move {
                        intake
                            .establish(session_id, connecting, session_span.clone())
                            .instrument(session_span)
                            .await;
                    });
                }
            })
            .unwrap()
    }

    fn is_endpoint_closing(error: &anyhow::Error) -> bool {
        error
            .chain()
            .any(|cause| cause.to_string() == "Endpoint is closing")
    }
}

impl Drop for SessionHandler {
    fn drop(&mut self) {
        tracing::info!("Handle dropped.");
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Context;

    use super::SessionHandler;
    use crate::modules::auth::{
        test_support::{client_endpoint, spawn_relay_with_verifier},
        verified_token::VerifiedToken,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_client_setup_does_not_block_new_connections() {
        // Arrange: client A finishes the QUIC handshake but never sends CLIENT_SETUP
        let relay = spawn_relay_with_verifier(VerifiedToken::anonymous()).await;
        let url = format!("moqt://127.0.0.1:{}", relay.port);
        let stalled_endpoint = client_endpoint(None);
        let _stalled_setup = stalled_endpoint.connect(&url).await.unwrap();

        // Act
        let endpoint = client_endpoint(None);
        let session = tokio::time::timeout(Duration::from_secs(2), async {
            endpoint.connect(&url).await?.await
        })
        .await;

        // Assert
        assert!(
            session.is_ok_and(|result| result.is_ok()),
            "a stalled CLIENT_SETUP blocked the accept loop"
        );
    }

    #[test]
    fn endpoint_closing_error_stops_accept_loop() {
        // Arrange
        let error = anyhow::anyhow!("Endpoint is closing");

        // Act
        let should_stop = SessionHandler::is_endpoint_closing(&error);

        // Assert
        assert!(should_stop);
    }

    #[test]
    fn wrapped_endpoint_closing_error_stops_accept_loop() {
        // Arrange
        let error = Err::<(), _>(anyhow::anyhow!("Endpoint is closing"))
            .context("accept failed")
            .unwrap_err();

        // Act
        let should_stop = SessionHandler::is_endpoint_closing(&error);

        // Assert
        assert!(should_stop);
    }

    #[test]
    fn connection_timeout_error_keeps_accept_loop_running() {
        // Arrange
        let error = anyhow::anyhow!("connection error: timed out");

        // Act
        let should_stop = SessionHandler::is_endpoint_closing(&error);

        // Assert
        assert!(!should_stop);
    }
}
