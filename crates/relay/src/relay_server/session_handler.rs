use std::sync::Arc;

use moqt::{AcceptError, Endpoint, TransportProtocol, server::Accepting};
use tokio::sync::oneshot;
use tracing::{Instrument, Span};

use crate::{
    logging::relay_hostname,
    modules::{
        auth::session_authenticator::SessionAuthenticator,
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

#[derive(Clone)]
pub(crate) struct SessionIntake {
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) relay_session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    pub(crate) accepted_peer: SessionPeer,
    pub(crate) authenticator: Arc<SessionAuthenticator>,
}

impl SessionIntake {
    async fn establish(&self, session_id: SessionId, connecting: Accepting, session_span: Span) {
        let handshake = match connecting.await {
            Ok(handshake) => handshake,
            Err(error) => {
                tracing::warn!(%error, "failed to establish session");
                return;
            }
        };
        let verified_token = match self
            .authenticator
            .authenticate(handshake.authorization_tokens(), &self.accepted_peer)
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
                        peer: self.accepted_peer,
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
    stop_sender: Option<oneshot::Sender<()>>,
    #[cfg(test)]
    pub(crate) local_port: u16,
}

impl SessionHandler {
    pub(crate) fn run<T: TransportProtocol>(
        config: moqt::server::ServerConfig,
        intake: SessionIntake,
    ) -> Self {
        let endpoint = Endpoint::<T>::create_server(&config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        #[cfg(test)]
        let local_port = endpoint
            .local_port()
            .expect("a server endpoint is bound to a UDP socket");
        let (stop_sender, mut stop_receiver) = oneshot::channel();
        let join_handle = tokio::task::Builder::new()
            .spawn(async move {
                let relay_hostname = relay_hostname();
                loop {
                    let session_id = generate_session_id();
                    let session_peer = intake.accepted_peer.kind();
                    let session_span = tracing::info_span!(
                        parent: None,
                        "relay.session",
                        session_id = session_id,
                        session_peer = session_peer,
                        relay_hostname = %relay_hostname,
                    );
                    let accept = async {
                        endpoint.accept().await.inspect_err(|error| match error {
                            AcceptError::EndpointClosing => {
                                tracing::info!(%error, "transport endpoint closed");
                            }
                            _ => tracing::warn!(%error, "failed to accept transport connection"),
                        })
                    }
                    .instrument(session_span.clone());
                    let connecting = tokio::select! {
                        biased;
                        _ = &mut stop_receiver => {
                            tracing::info!(peer = session_peer, "stopped accepting sessions; refusing new connections");
                            endpoint.refuse_new_connections();
                            break;
                        }
                        connecting = accept => connecting,
                    };
                    let connecting = match connecting {
                        Ok(connecting) => connecting,
                        Err(AcceptError::EndpointClosing) => break,
                        Err(_) => continue,
                    };

                    let intake = intake.clone();
                    tokio::spawn(async move {
                        intake
                            .establish(session_id, connecting, session_span.clone())
                            .instrument(session_span)
                            .await;
                    });
                }
                // Arrivals are refused at the transport until the sessions
                // already accepted have all closed.
                tokio::select! {
                    _ = async {
                        while endpoint.accept().await.is_ok() {}
                    } => {}
                    _ = endpoint.wait_idle() => {}
                }
            })
            .unwrap();
        Self {
            join_handle,
            stop_sender: Some(stop_sender),
            #[cfg(test)]
            local_port,
        }
    }

    /// Ends the accept loop and refuses every later connection attempt at the
    /// transport; the sessions already accepted keep running.
    pub fn stop_accepting(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
    }

    /// Resolves once the accept loop has stopped and every transport
    /// connection of the endpoint has finished closing, so the process can
    /// exit without losing the CONNECTION_CLOSE of a session closed last.
    /// Call `stop_accepting` first; otherwise this never resolves.
    pub async fn wait_idle(&mut self) {
        let _ = (&mut self.join_handle).await;
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

    use crate::modules::auth::{
        test_support::{
            client_endpoint, spawn_relay_and_connect_client, spawn_relay_with_verifier,
        },
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stopped_handler_refuses_new_connections_at_once() {
        // Arrange: one session keeps the endpoint alive while accepting stops
        let (mut relay, _established) =
            spawn_relay_and_connect_client(VerifiedToken::anonymous()).await;
        let url = format!("moqt://127.0.0.1:{}", relay.port);
        relay.handler.stop_accepting();

        // Act
        let endpoint = client_endpoint(None);
        let connect = tokio::time::timeout(Duration::from_secs(2), async {
            endpoint.connect(&url).await?.await
        })
        .await;

        // Assert
        let result = connect.expect("the refusal must arrive within the timeout");
        assert!(result.is_err());
    }
}
