use std::{
    net::ToSocketAddrs,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use moqt::{DatagramField, Endpoint, Session, SubscribeOption, TransportProtocol};

use crate::stream_runner::StreamTaskRunner;

// Dead code is used for testing publish and datagram functionalities,
// which are not used in the current test cases but may be used in future test cases.
// Therefore, we will keep the dead code for now.
// We can remove the dead code when we have implemented the test cases that use these functionalities.

#[allow(dead_code)]
pub struct Client<T: TransportProtocol> {
    label: String,
    join_handle: tokio::task::JoinHandle<()>,
    track_alias: Arc<AtomicU64>,
    session: Arc<Session<T>>,
    joinset: tokio::task::JoinSet<()>,
}

impl<T: TransportProtocol> Client<T> {
    pub async fn new(cert_path: String, label: String) -> anyhow::Result<Self> {
        let endpoint = Endpoint::<T>::create_client_with_custom_cert(0, &cert_path)?;
        let url = url::Url::from_str("moqt://localhost:4433")?;
        let host = url.host_str().unwrap();
        let remote_address = (host, url.port().unwrap_or(4433))
            .to_socket_addrs()?
            .next()
            .unwrap();

        tracing::info!("remote_address: {} host: {}", remote_address, host);

        let connecting = endpoint.connect(remote_address, host).await?;
        let session = connecting.await?;

        let track_alias = Arc::new(AtomicU64::new(0));
        let session = Arc::new(session);
        let join_handle =
            Self::create_receiver(label.clone(), session.clone(), track_alias.clone());

        Ok(Self {
            label,
            join_handle,
            track_alias,
            session,
            joinset: tokio::task::JoinSet::new(),
        })
    }

    pub fn create_receiver(
        // pub(crate) -> pub
        _label: String,
        session: Arc<Session<T>>,
        _track_alias: Arc<AtomicU64>,
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
                                _label,
                                publish_namespace_handler.track_namespace
                            );
                            let _ = publish_namespace_handler.ok().await;
                        }
                        moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                            tracing::info!(
                                "Received: {} Subscribe Namespace: {}",
                                _label,
                                subscribe_namespace_handler.track_namespace_prefix
                            );
                            let _ = subscribe_namespace_handler.ok().await;
                        }
                        moqt::SessionEvent::Publish(publish_handler) => {
                            tracing::info!("Received: {} Publish", _label);
                            let _subscription = match publish_handler
                                .ok(128, moqt::FilterType::LargestObject, 0)
                                .await
                            {
                                Ok(subscription) => subscription,
                                Err(_) => {
                                    tracing::error!("failed to send");
                                    return;
                                }
                            };
                        }
                        moqt::SessionEvent::Subscribe(subscribe_handler) => {
                            tracing::info!("Received: {} Subscribe", _label);
                            let track_alias = _track_alias.load(Ordering::SeqCst);
                            let _ = subscribe_handler
                                .ok(1000000, moqt::ContentExists::False)
                                .await;
                            let publication = subscribe_handler.into_publication(track_alias);
                            Self::create_stream(_label.clone(), session.clone(), publication, &runner)
                                .await;
                        }
                        moqt::SessionEvent::Unsubscribe(unsubscribe_handler) => {
                            tracing::info!(
                                "Received: {} Unsubscribe {}",
                                _label,
                                unsubscribe_handler.subscribe_id()
                            );
                        }
                        moqt::SessionEvent::Disconnected() => {
                            tracing::info!("Received: {} Disconnected", _label);
                            break;
                        }
                        moqt::SessionEvent::ProtocolViolation() => {
                            tracing::info!("Received: {} ProtocolViolation", _label);
                        }
                    };
                }
            })
            .unwrap()
    }

    pub async fn publish_namespace(&self, track_namespace: String) -> anyhow::Result<()> {
        // pub(crate) -> pub
        let result = self
            .session
            .publisher()
            .publish_namespace(track_namespace)
            .await;
        if result.is_err() {
            tracing::info!("{}: publish namespace error", self.label);
            return Err(anyhow::anyhow!("Publish namespace error"));
        } else {
            tracing::info!("{}: publish namespace ok", self.label);
        }
        Ok(())
    }

    pub async fn subscribe_namespace(&self, track_namespace_prefix: String) -> anyhow::Result<()> {
        // pub(crate) -> pub
        let result = self
            .session
            .subscriber()
            .subscribe_namespace(track_namespace_prefix)
            .await;
        if result.is_err() {
            tracing::info!("{}: subscribe namespace error", self.label);
            return Err(anyhow::anyhow!("Subscribe namespace error"));
        } else {
            tracing::info!("{}: subscribe namespace ok", self.label);
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn publish(&self, track_namespace: String, track_name: String) {
        // pub(crate) -> pub
        let option = moqt::PublishOption::default();
        let pub_result = self
            .session
            .publisher()
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

    // async fn subscribe(
    //     label: String,
    //     publish_handler: moqt::PublishHandler<moqt::QUIC>,
    //     runner: &StreamTaskRunner,
    // ) {
    //     let full_name = format!(
    //         "{}/{}",
    //         publish_handler.track_namespace, publish_handler.track_name
    //     );
    //     let task = async move {
    //         tracing::info!("{} :subscribe {}", label, full_name);
    //         let subscription = publish_handler.into_subscription(0);
    //         let receiver = match subscription.accept_data_receiver().await {
    //             Ok(receiver) => receiver,
    //             Err(_) => {
    //                 tracing::error!("Failed to accept stream or datagram");
    //                 return;
    //             }
    //         };
    //         match receiver {
    //             moqt::DataReceiver::Stream(mut stream) => loop {
    //                 let result = match stream.receive().await {
    //                     Ok(r) => r,
    //                     Err(e) => {
    //                         tracing::error!("Failed to receive: {}", e);
    //                         break;
    //                     }
    //                 };
    //                 tracing::info!("{} :subscribe stream: {:?}", label, result);
    //             },
    //             moqt::DataReceiver::Datagram(mut datagram) => loop {
    //                 let result = match datagram.receive().await {
    //                     Ok(r) => r,
    //                     Err(e) => {
    //                         tracing::error!("Failed to receive: {}", e);
    //                         break;
    //                     }
    //                 };
    //                 tracing::info!("{} :subscribe datagram: {:?}", label, result);
    //             },
    //         }
    //     };
    //     runner.add_task(Box::pin(task)).await;
    // }

    async fn create_stream(
        label: String,
        session: Arc<Session<T>>,
        publication: moqt::PublishedResource,
        runner: &StreamTaskRunner,
    ) {
        tracing::info!("{} :create stream", label);
        let task = async move {
            let mut group_id = 0;
            let mut id = 0;
            tracing::info!("{} :create stream start", label);
            let stream_factory = session.publisher().create_stream(&publication);
            while let Ok(uninit_stream) = stream_factory.next().await {
                tracing::info!("group_id={} :stream created", group_id);
                let header = uninit_stream.create_header(
                    group_id,
                    moqt::SubgroupId::None,
                    128,
                    false,
                    false,
                );
                let mut stream = match uninit_stream.send_header(header).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("failed to send header: {}", e);
                        break;
                    }
                };
                while id < 10 {
                    let format_text = format!("hello from {}! id: {}", label, id);
                    let data = moqt::SubgroupObject::new_payload(format_text.into());
                    let extension_headers = moqt::ExtensionHeaders {
                        prior_group_id_gap: vec![],
                        prior_object_id_gap: vec![],
                        immutable_extensions: vec![],
                    };
                    let obj = stream.create_object_field(id, extension_headers, data);
                    match stream.send(obj).await {
                        Ok(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                            id += 1;
                        }
                        Err(e) => {
                            tracing::error!("failed to send: {}", e);
                            break;
                        }
                    }
                }
                group_id += 1;
                id = 0;
            }
        };
        runner.add_task(Box::pin(task)).await;
    }

    #[allow(dead_code)]
    async fn create_datagram(
        label: String,
        publisher: &moqt::Publisher<T>,
        publication: moqt::PublishedResource,
        runner: &StreamTaskRunner,
    ) {
        tracing::info!("{} :create datagram", label);
        let mut datagram = publisher.create_datagram(&publication);
        // let mut datagram = publication.create_stream().await.unwrap();
        let task = async move {
            let mut id = 0;
            tracing::info!("{} :create datagram start", label);
            loop {
                let format_text = format!("hello from {}! id: {}", label, id);
                let data = DatagramField::to_bytes(format_text);
                let field = DatagramField::Payload0x00 {
                    object_id: id,
                    publisher_priority: 128,
                    payload: data,
                };
                let obj = datagram.create_object_datagram(id, field);
                match datagram.send(obj).await {
                    Ok(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                        id += 1
                    }
                    Err(_) => {
                        tracing::error!("failed to send");
                        break;
                    }
                }
            }
        };
        runner.add_task(Box::pin(task)).await;
    }

    pub async fn active_subscribe(
        // pub(crate) -> pub
        &mut self,
        label: String,
        track_namespace: String,
        track_name: String,
    ) {
        let option = SubscribeOption {
            subscriber_priority: 128,
            group_order: moqt::GroupOrder::Ascending,
            forward: true,
            filter_type: moqt::FilterType::LargestObject,
        };
        let mut subscriber = self.session.subscriber();
        let subscription = match subscriber
            .subscribe(track_namespace, track_name, option)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to subscribe: {}", e);
                return;
            }
        };
        loop {
            tokio::select! {
                Ok(result) = subscriber.accept_data_receiver(&subscription) => {
                    match result {
                        moqt::DataReceiver::Stream(mut factory) => {
                            let label = label.clone();
                            self.joinset.spawn(async move {
                                while let Ok(mut stream) = factory.next().await {
                                    while let Ok(result) = stream.receive().await {
                                        tracing::info!("{} :active subscribe stream: {:?}", label, result);
                                    }
                                }
                            });
                        }
                        moqt::DataReceiver::Datagram(mut datagram) => {
                            let label = label.clone();
                            self.joinset.spawn(async move {
                                while let Ok(result) = datagram.receive().await {
                                    tracing::info!("{} :active subscribe datagram: {:?}", label, result);
                                }
                            });
                        },
                    }
                }
                Ok(()) = tokio::signal::ctrl_c() => {
                    tracing::info!("{} :active subscribe received shutdown signal", label);
                    break;
                }
                Some(_) = self.joinset.join_next() => {
                    tracing::info!("{} :active subscribe task finished", label);
                }
            }
        }
    }
}

impl<T: TransportProtocol> Drop for Client<T> {
    fn drop(&mut self) {
        tracing::info!("Client dropped.");
        self.join_handle.abort();
    }
}
