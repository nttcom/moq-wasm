use std::{net::ToSocketAddrs, str::FromStr, sync::Arc};

use moqt::ClientConfig;

use crate::{StreamType, message_receive_thread::MessageReceiveThread};

pub(crate) struct MOQTClient {
    session: Arc<moqt::Session<moqt::QUIC>>,
    message_receive_thread: tokio::task::JoinHandle<()>,
}

impl MOQTClient {
    pub(crate) async fn new(
        cert_path: &str,
        sender: tokio::sync::mpsc::Sender<StreamType>,
    ) -> anyhow::Result<Self> {
        // let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client_with_custom_cert(0, cert_path)?;
        let url = url::Url::from_str("moqt://localhost:4434")?;
        let mut config = ClientConfig::default();
        config.verify_certificate = false;
        let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client(&config)?;
        let url = url::Url::from_str("moqt://moqt.research.skyway.io:4434")?;
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
            .publisher()
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
