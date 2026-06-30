use std::sync::Arc;

use moqt::{Endpoint, TransportProtocol};
use tracing::Instrument;

use crate::modules::{
    session_event::SessionEvent,
    session_repository::{SessionPeer, SessionRepository},
    types::generate_session_id,
};

fn relay_hostname() -> String {
    std::env::var("RELAY_HOSTNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub struct SessionHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionHandler {
    pub(crate) fn run<T: TransportProtocol>(
        config: moqt::ServerConfig,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        accepted_peer: SessionPeer,
    ) -> Self {
        let endpoint = Endpoint::<T>::create_server(&config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        let join_handle =
            Self::create_joinhandle::<T>(endpoint, repo, relay_session_event_sender, accepted_peer);
        Self { join_handle }
    }

    fn create_joinhandle<T: TransportProtocol>(
        mut endpoint: Endpoint<T>,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        accepted_peer: SessionPeer,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                let relay_hostname = relay_hostname();
                loop {
                    let session_id = generate_session_id();
                    let session_peer = accepted_peer.kind();
                    let session_peer_relay_id = accepted_peer.relay_id().unwrap_or("unknown");
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
                    let session = async {
                        connecting.await.inspect_err(|error| {
                            tracing::warn!(%error, "failed to establish session");
                        })
                    }
                    .instrument(session_span.clone())
                    .await;
                    let session = match session {
                        Ok(session) => session,
                        Err(_) => continue,
                    };
                    let session_add_span = tracing::info_span!(
                        parent: &session_span,
                        "relay.session_repository.add",
                        session_id = session_id
                    );

                    async {
                        tracing::info!("Session accepted");
                        let mut repo = repo.lock().await;
                        match accepted_peer.clone() {
                            SessionPeer::Client => {
                                repo.add_client(
                                    session_id,
                                    Box::new(session),
                                    relay_session_event_sender.clone(),
                                    session_span.clone(),
                                )
                                .await;
                            }
                            SessionPeer::Relay { relay_id } => {
                                repo.add_relay(
                                    session_id,
                                    Box::new(session),
                                    relay_session_event_sender.clone(),
                                    session_span.clone(),
                                    relay_id,
                                )
                                .await;
                            }
                        }
                    }
                    .instrument(session_add_span)
                    .await;
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
    use anyhow::Context;

    use super::SessionHandler;

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
