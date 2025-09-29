use moqt::{Endpoint, QUIC};

use crate::modules::{publisher::Publisher, subscriber::Subscriber};

struct Handler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl Handler {
    pub(crate) fn run(
        key_path: String,
        cert_path: String,
        event_sender: tokio::sync::mpsc::Sender<(Publisher, Subscriber)>,
    ) {
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
        Self { join_handle };
    }

    fn create_joinhandle(
        mut endpoint: Endpoint<QUIC>,
        event_sender: tokio::sync::mpsc::Sender<(Publisher, Subscriber)>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                let mut pubsub_id = 0;
                loop {
                    let session = match endpoint.accept().await.inspect_err(|e| {
                        tracing::error!("failed to accept: {}", e);
                    }) {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                    let publisher = session.create_publisher();
                    let subscriber = session.create_subscriber();
                    let publisher = Publisher { id: pubsub_id, session_id: session.id, publisher };
                    let subscriber = Subscriber { id: pubsub_id, session_id: session.id, subscriber };
                    event_sender.send((publisher, subscriber));
                    pubsub_id += 1;
                }
            })
            .unwrap()
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
