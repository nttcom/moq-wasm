use std::sync::Arc;

use moqt::{Endpoint, QUIC};
use uuid::Uuid;

use crate::modules::{
    enums::SessionEvent, repositories::session_repository::SessionRepository
};

pub struct Handler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl Handler {
    pub fn run(
        key_path: String,
        cert_path: String,
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>
    ) -> Self {
        let config = moqt::ServerConfig {
            port: 4433,
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
        session_event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                loop {
                    tracing::info!("accepting...");
                    let session = match endpoint.accept().await.inspect_err(|e| {
                        tracing::error!("failed to accept: {}", e);
                    }) {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                    let session_id = Uuid::new_v4();
                    tracing::info!("Session ID: {}", session_id);
                    repo.lock().await.add(session_id, Box::new(session), session_event_sender.clone()).await;
                }
            })
            .unwrap()
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        tracing::info!("Handle has been dropped.");
        self.join_handle.abort();
    }
}
