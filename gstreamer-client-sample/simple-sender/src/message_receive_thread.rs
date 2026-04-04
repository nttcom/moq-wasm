use std::sync::Arc;

use crate::StreamType;

pub(crate) struct MessageReceiveThread;

impl MessageReceiveThread {
    pub(crate) fn start(
        session: Arc<moqt::Session<moqt::QUIC>>,
        sender: tokio::sync::mpsc::Sender<StreamType>,
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
                            .ok(128, moqt::FilterType::LargestObject)
                            .await
                        {
                            Ok(()) => {}
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
                            Ok(alias) => alias,
                            Err(e) => {
                                tracing::error!("failed to send subscribe ok: {}", e);
                                return;
                            }
                        };
                        let published_resource = subscribe_handler.into_publication(track_alias);
                        let stream = Self::stream(session.clone(), &published_resource)
                            .await
                            .expect("failed to create stream");
                        let _ = sender.send(stream).await;
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
            Ok(session.publisher().create_stream(published_resource))
        }
        #[cfg(feature = "use_datagram")]
        {
            Ok(session.publisher().create_datagram(published_resource))
        }
    }
}
