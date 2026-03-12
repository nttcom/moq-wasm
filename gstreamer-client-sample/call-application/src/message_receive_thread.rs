use std::sync::Arc;

use crate::{MOQTEvent, StreamType};

pub(crate) struct MessageReceiveThread;

impl MessageReceiveThread {
    pub(crate) fn start(
        session: Arc<moqt::Session<moqt::QUIC>>,
        sender: tokio::sync::mpsc::Sender<MOQTEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn(async move {
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
                            "Received! Publish Namespace: {}",
                            publish_namespace_handler.track_namespace
                        );
                        let _ = publish_namespace_handler.ok().await;
                        let namespace = publish_namespace_handler.track_namespace;
                        let event = MOQTEvent::NamespaceAdded(namespace);
                        sender.send(event).await.unwrap_or_else(|e| {
                            tracing::error!("Failed to send track namespace: {}", e);
                        });
                    }
                    moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                        tracing::info!(
                            "Received! Subscribe Namespace: {}",
                            subscribe_namespace_handler.track_namespace_prefix
                        );
                        let _ = subscribe_namespace_handler.ok().await;
                    }
                    moqt::SessionEvent::Publish(publish_handler) => {
                        tracing::info!("Received! Publish");
                        match publish_handler
                            .ok(128, moqt::FilterType::LatestObject)
                            .await
                        {
                            Ok(h) => h,
                            Err(_) => {
                                tracing::error!("failed to send");
                                return;
                            }
                        }
                    }
                    moqt::SessionEvent::Subscribe(subscribe_handler) => {
                        tracing::info!("Received! Subscribe");
                        let track_alias = match subscribe_handler
                            .ok(1000000, moqt::ContentExists::False)
                            .await
                        {
                            Ok(track_alias) => track_alias,
                            Err(e) => {
                                tracing::error!("Failed to send SubscribeOk: {}", e);
                                continue;
                            }
                        };
                        let published_resource = subscribe_handler.into_publication(track_alias);
                        let stream = Self::stream(session.clone(), &published_resource)
                            .await
                            .expect("failed to create stream");
                        let is_video = subscribe_handler.track_name == "video";
                        let _ = sender
                            .send(MOQTEvent::StreamAdded { is_video, stream })
                            .await;
                    }
                    moqt::SessionEvent::ProtocolViolation() => {
                        tracing::info!("Received: ProtocolViolation");
                    }
                };
            }
        })
    }

    async fn stream(
        session: Arc<moqt::Session<moqt::QUIC>>,
        published_resource: &moqt::PublishedResource,
    ) -> anyhow::Result<StreamType> {
        #[cfg(not(feature = "use_datagram"))]
        {
            session.publisher().create_stream(published_resource).await
        }
        #[cfg(feature = "use_datagram")]
        {
            Ok(session.publisher().create_datagram(published_resource))
        }
    }
}
