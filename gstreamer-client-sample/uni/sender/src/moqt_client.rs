use std::{net::ToSocketAddrs, str::FromStr, sync::Arc};

use crate::message_receive_thread::MessageReceiveThread;

pub(crate) struct MOQTClient {
    session: Arc<moqt::Session<moqt::QUIC>>,
    message_receive_thread: tokio::task::JoinHandle<()>,
}

impl MOQTClient {
    pub(crate) async fn new(
        cert_path: &str,
        sender: tokio::sync::mpsc::Sender<moqt::StreamDataSender<moqt::QUIC>>,
    ) -> anyhow::Result<Self> {
        let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client_with_custom_cert(0, cert_path)?;
        let url = url::Url::from_str("moqt://localhost:4434")?; // ここも変更
        let host = url.host_str().unwrap();
        let remote_address = (host, url.port().unwrap_or(4433))
            .to_socket_addrs()?
            .next()
            .unwrap();
        let session = endpoint.connect(remote_address, host).await?;
        let session = Arc::new(session);
        let message_receive_thread = MessageReceiveThread::start(Arc::clone(&session), sender);

        Ok(Self {
            session,
            message_receive_thread,
        })
    }

    pub(crate) async fn publish_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.session
            .create_publisher()
            .publish_namespace(track_namespace.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to publish namespace: {}", e))
    }
}

impl Drop for MOQTClient {
    fn drop(&mut self) {
        self.message_receive_thread.abort();
    }
}
