use moqt::{Endpoint, QUIC};
use uuid::Uuid;

use crate::modules::{
    core::{publisher::Publisher, session::Session, subscriber::Subscriber},
    types::SessionId,
};

pub struct Handler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl Handler {
    pub fn run(
        key_path: String,
        cert_path: String,
        event_sender: tokio::sync::mpsc::UnboundedSender<(
            SessionId,
            Box<dyn Session>,
            Box<dyn Publisher>,
            Box<dyn Subscriber>,
        )>,
    ) -> Self {
        let config = moqt::ServerConfig {
            port: 4433,
            cert_path,
            key_path,
            keep_alive_interval_sec: 30,
        };
        let endpoint = Endpoint::<QUIC>::create_server(config)
            .inspect_err(|e| tracing::error!("failed to create server: {}", e))
            .unwrap();
        let join_handle = Self::create_joinhandle(endpoint, event_sender);
        Self { join_handle }
    }

    fn create_joinhandle(
        mut endpoint: Endpoint<QUIC>,
        event_sender: tokio::sync::mpsc::UnboundedSender<(
            SessionId,
            Box<dyn Session>,
            Box<dyn Publisher>,
            Box<dyn Subscriber>,
        )>,
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
                    let (publisher, subscriber) = session.new_publisher_subscriber_pair();
                    let session_id = Uuid::new_v4();
                    tracing::info!("Session ID: {}", session_id);
                    event_sender.send((session_id, Box::new(session), publisher, subscriber));
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
