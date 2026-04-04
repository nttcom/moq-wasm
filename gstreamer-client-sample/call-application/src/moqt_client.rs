use std::{net::ToSocketAddrs, str::FromStr, sync::Arc};

use moqt::ClientConfig;

use crate::{MOQTEvent, message_receive_thread::MessageReceiveThread};

pub(crate) struct MOQTClient {
    session: Arc<moqt::Session<moqt::QUIC>>,
    message_receive_thread: tokio::task::JoinHandle<()>,
}

impl MOQTClient {
    // `cert_path` is not used in this sample because `verify_certificate` is set to false, but you can specify it if you want to use it.
    #[allow(unused_variables)]
    pub(crate) async fn new(
        cert_path: &str,
        event_sender: tokio::sync::mpsc::Sender<MOQTEvent>,
    ) -> anyhow::Result<Self> {
        // let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client_with_custom_cert(0, cert_path)?;
        // let url = url::Url::from_str("moqt://localhost:4434")?;
        let mut config = ClientConfig {
            port: 0,
            verify_certificate: false,
        };
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
        let message_receive_thread =
            MessageReceiveThread::start(Arc::clone(&session), event_sender);

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

    pub(crate) async fn subscribe_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.session
            .subscriber()
            .subscribe_namespace(track_namespace.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to subscribe namespace: {}", e))
    }

    pub(crate) async fn subscribe(
        &self,
        track_namespace: &str,
        track_name: &str,
        subscribe_options: moqt::SubscribeOption,
    ) -> anyhow::Result<moqt::DataReceiver<moqt::QUIC>> {
        let mut subscriber = self.session.subscriber();
        let subscription = subscriber
            .subscribe(
                track_namespace.to_string(),
                track_name.to_string(),
                subscribe_options,
            )
            .await?;
        subscriber.accept_data_receiver(&subscription).await
    }
}

impl Drop for MOQTClient {
    fn drop(&mut self) {
        self.message_receive_thread.abort();
    }
}
