use std::sync::Arc;

use moqt::{Endpoint, TransportProtocol};
use tracing::Instrument;

use crate::modules::{
    enums::MoqtRelayEvent, session_repository::SessionRepository, types::generate_session_id,
};

pub struct SessionHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionHandler {
    pub(crate) fn run<T: TransportProtocol>(
        config: moqt::ServerConfig,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_event_sender: tokio::sync::mpsc::UnboundedSender<MoqtRelayEvent>,
    ) -> Self {
        let endpoint = Endpoint::<T>::create_server(&config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        let join_handle = Self::create_joinhandle::<T>(endpoint, repo, relay_event_sender);
        Self { join_handle }
    }

    fn create_joinhandle<T: TransportProtocol>(
        mut endpoint: Endpoint<T>,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        relay_event_sender: tokio::sync::mpsc::UnboundedSender<MoqtRelayEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                loop {
                    let session_id = generate_session_id();
                    let session_span =
                        tracing::info_span!(parent: None, "MoQTSession", session_id = session_id);
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
                        repo.lock()
                            .await
                            .add(
                                session_id,
                                Box::new(session),
                                relay_event_sender.clone(),
                                session_span.clone(),
                            )
                            .await;
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
