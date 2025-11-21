use std::sync::Arc;

use moqt::{Endpoint, QUIC};
use uuid::Uuid;

use crate::modules::{
    enums::MOQTMessageReceived, repositories::session_repository::SessionRepository,
};

pub struct SessionHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SessionHandler {
    pub fn run(
        key_path: String,
        cert_path: String,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<MOQTMessageReceived>,
    ) -> Self {
        let config = moqt::ServerConfig {
            port: 4434,
            cert_path,
            key_path,
            keep_alive_interval_sec: 15,
        };
        let endpoint = Endpoint::<QUIC>::create_server(config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        let join_handle = Self::create_joinhandle(endpoint, repo, session_event_sender);
        Self { join_handle }
    }

    fn create_joinhandle(
        mut endpoint: Endpoint<QUIC>,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<MOQTMessageReceived>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                loop {
                    tracing::info!("accepting...");
                    let session = match endpoint.accept().await.inspect_err(|e| {
                        tracing::error!("failed to accept: {}", e);
                    }) {
                        Ok(s) => s,
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            break;
                        }
                    };
                    let session_id = Uuid::new_v4();
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
        tracing::info!("Handle has been dropped.");
        self.join_handle.abort();
    }
}
