use std::{
    net::ToSocketAddrs,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::bail;
use moqt::{DatagramHeader, Endpoint, QUIC, Session};

use crate::stream_runner::StreamTaskRunner;

pub(crate) struct Client {
    label: String,
    join_handle: tokio::task::JoinHandle<()>,
    track_alias: Arc<AtomicU64>,
    publisher: moqt::Publisher<moqt::QUIC>,
    subscriber: moqt::Subscriber<moqt::QUIC>,
}

impl Client {
    pub(crate) async fn new(cert_path: String, label: String) -> anyhow::Result<Self> {
        let endpoint = Endpoint::<QUIC>::create_client_with_custom_cert(0, &cert_path)?;
        let url = url::Url::from_str("moqt://localhost:4433")?;
        let host = url.host_str().unwrap();
        let remote_address = (host, url.port().unwrap_or(4433))
            .to_socket_addrs()?
            .next()
            .unwrap();

        tracing::info!("remote_address: {} host: {}", remote_address, host);

        let session = match endpoint.connect(remote_address, host).await {
            Ok(s) => s,
            Err(e) => {
                bail!("test failed: {:?}", e)
            }
        };
        let track_alias = Arc::new(AtomicU64::new(0));
        let (publisher, subscriber) = session.create_publisher_subscriber_pair();
        let join_handle = Self::create_receiver(label.clone(), session, track_alias.clone());

        Ok(Self {
            label,
            join_handle,
            track_alias,
            publisher,
            subscriber,
        })
    }

    fn create_receiver(
        label: String,
        session: Session<moqt::QUIC>,
        track_alias: Arc<AtomicU64>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .spawn(async move {
                let runner = StreamTaskRunner::new();
                loop {
                    let result = session.receive_event().await;
                    if let Err(e) = result {
                        tracing::error!("Failed to receive event: {}", e);
                        break;
                    }
                    let event = result.unwrap();
                    match event {
                        moqt::SessionEvent::PublishNamespace(publish_namespace_handler) => {
                            tracing::info!(
                                "Received: {} Publish Namespace: {}",
                                label,
                                publish_namespace_handler.track_namespace
                            );
                            let _ = publish_namespace_handler.ok().await;
                        }
                        moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                            tracing::info!(
                                "Received: {} Subscribe Namespace: {}",
                                label,
                                subscribe_namespace_handler.track_namespace_prefix
                            );
                            let _ = subscribe_namespace_handler.ok().await;
                        }
                        moqt::SessionEvent::Publish(publish_handler) => {
                            tracing::info!("Received: {} Publish", label);
                            match publish_handler
                                .ok(128, moqt::FilterType::LatestObject)
                                .await
                            {
                                Ok(h) => h,
                                Err(_) => {
                                    tracing::error!("failed to send");
                                    return;
                                }
                            };
                            Self::subscribe(label.clone(), &publish_handler, &runner).await;
                        }
                        moqt::SessionEvent::Subscribe(subscribe_handler) => {
                            tracing::info!("Received: {} Subscribe", label);
                            let track_alias = track_alias.load(Ordering::SeqCst);
                            let _ = subscribe_handler.ok(track_alias, 1000000, false).await;
                            let publication = subscribe_handler.into_publication(track_alias);
                            Self::create_stream(label.clone(), publication, &runner).await;
                        }
                        moqt::SessionEvent::ProtocolViolation() => {
                            tracing::info!("Received: {} ProtocolViolation", label);
                        }
                    };
                }
            })
            .unwrap()
    }

    pub(crate) async fn publish_namespace(&self, track_namespace: String) {
        let result = self.publisher.publish_namespace(track_namespace).await;
        if result.is_err() {
            tracing::info!("{}: publish namespace error", self.label);
        } else {
            tracing::info!("{}: publish namespace ok", self.label);
        }
    }

    pub(crate) async fn subscribe_namespace(&self, track_namespace_prefix: String) {
        let result = self
            .subscriber
            .subscribe_namespace(track_namespace_prefix)
            .await;
        if result.is_err() {
            tracing::info!("{}: subscribe namespace error", self.label);
        } else {
            tracing::info!("{}: subscribe namespace ok", self.label);
        }
    }

    pub(crate) async fn publish(&self, track_namespace: String, track_name: String) {
        let option = moqt::PublishOption::default();
        let pub_result = self
            .publisher
            .publish(track_namespace, track_name, option)
            .await;
        if let Ok(p) = pub_result {
            tracing::info!("{}: publish ok", self.label);
            self.track_alias
                .fetch_add(p.track_alias, std::sync::atomic::Ordering::SeqCst);
        } else {
            tracing::error!("{}: publish error", self.label);
        }
    }

    async fn subscribe(
        label: String,
        publish_handler: &moqt::PublishHandler<moqt::QUIC>,
        runner: &StreamTaskRunner,
    ) {
        let full_name = format!(
            "{}/{}",
            publish_handler.track_namespace, publish_handler.track_name
        );
        tracing::info!("{} :subscribe {}", label, full_name);
        let option = moqt::SubscribeOption {
            subscriber_priority: 128,
            group_order: publish_handler.group_order,
            forward: publish_handler.forward,
            filter_type: moqt::FilterType::LatestObject,
            start_location: None,
            end_group: None,
        };
        let subscription = publish_handler
            .subscribe(
                publish_handler.track_namespace.to_string(),
                publish_handler.track_name.to_string(),
                option,
            )
            .await;
        if subscription.is_err() {
            tracing::error!("{} :subscribe error", label);
        } else {
            tracing::info!("{} :subscribe ok", label);
        }
        let subscription = subscription.unwrap();
        let task = async move {
            let acceptance = match subscription.accept_stream_or_datagram().await {
                Ok(acceptance) => acceptance,
                Err(_) => {
                    tracing::error!("Failed to accept stream or datagram");
                    return;
                }
            };
            match acceptance {
                moqt::Acceptance::Stream(stream) => todo!(),
                moqt::Acceptance::Datagram(mut receiver, object) => {
                    let text = String::from_utf8(object.object_payload.to_vec()).unwrap();
                    tracing::info!(
                        "{} :subscribe datagram] track_alias:{}, message: {}",
                        label,
                        object.track_alias,
                        text
                    );
                    loop {
                        let result = receiver.receive().await;
                        if let Err(e) = result {
                            tracing::error!("Failed to receive: {}", e);
                            break;
                        }
                        let object = result.unwrap();
                        let text = String::from_utf8(object.object_payload.to_vec()).unwrap();
                        tracing::info!(
                            "{} :subscribe datagram] track_alias:{}, message: {}",
                            label,
                            object.track_alias,
                            text
                        );
                    }
                }
            }
        };
        runner.add_task(Box::pin(task)).await;
    }

    async fn create_stream(
        label: String,
        publication: moqt::Publication<moqt::QUIC>,
        runner: &StreamTaskRunner,
    ) {
        let datagram = publication.create_datagram();
        let task = async move {
            let mut id = 0;
            loop {
                let header = DatagramHeader {
                    group_id: id,
                    object_id: Some(id),
                    publisher_priority: 128,
                    prior_object_id_gap: None,
                    prior_group_id_gap: None,
                    immutable_extensions: vec![],
                };
                let format_text = format!("hello from {}! id: {}", label, id);
                let obj = datagram
                    .create_object_datagram(header, format_text.as_bytes())
                    .unwrap();
                match datagram.send(obj) {
                    Ok(_) => id += 1,
                    Err(_) => {
                        tracing::error!("failed to send");
                        break;
                    }
                }
            }
        };
        runner.add_task(Box::pin(task)).await;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        tracing::info!("Client has been dropped.");
        self.join_handle.abort();
    }
}
