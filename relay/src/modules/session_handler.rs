use std::sync::Arc;

use moqt::{Endpoint, TransportProtocol};

use crate::modules::{
    enums::MOQTMessageReceived, session_repository::SessionRepository, types::generate_session_id,
};

pub struct SessionHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionHandler {
    pub(crate) fn run<T: TransportProtocol>(
        config: moqt::ServerConfig,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<MOQTMessageReceived>,
    ) -> Self {
        let endpoint = Endpoint::<T>::create_server(&config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        let join_handle = Self::create_joinhandle::<T>(endpoint, repo, session_event_sender);
        Self { join_handle }
    }

    fn create_joinhandle<T: TransportProtocol>(
        mut endpoint: Endpoint<T>,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<MOQTMessageReceived>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                loop {
                    tracing::info!("accepting...");
                    let connecting = match endpoint.accept().await.inspect_err(|e| {
                        tracing::error!("failed to accept: {}", e);
                    }) {
                        Ok(s) => s,
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            break;
                        }
                    };
                    let session = match connecting.await.inspect_err(|e| {
                        tracing::error!("failed to negotiate: {}", e);
                    }) {
                        Ok(s) => s,
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            break;
                        }
                    };
                    let session_id = generate_session_id();
                    tracing::info!("Session ID: {}", session_id);
                    repo.lock()
                        .await
                        .add(session_id, Box::new(session), session_event_sender.clone())
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
