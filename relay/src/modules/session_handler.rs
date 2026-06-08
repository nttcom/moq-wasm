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
                    let Ok(connecting) = async {
                        endpoint.accept().await.inspect_err(|e| {
                            tracing::error!("failed to accept: {}", e);
                        })
                    }
                    .instrument(session_span.clone())
                    .await
                    else {
                        break;
                    };
                    let Ok(session) = async {
                        connecting.await.inspect_err(|e| {
                            tracing::error!("failed to accept: {}", e);
                        })
                    }
                    .instrument(session_span.clone())
                    .await
                    else {
                        break;
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
}

impl Drop for SessionHandler {
    fn drop(&mut self) {
        tracing::info!("Handle dropped.");
        self.join_handle.abort();
    }
}
