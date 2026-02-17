use std::sync::Arc;

pub(crate) struct MessageReceiveThread;

impl MessageReceiveThread {
    pub(crate) fn start(
        session: Arc<moqt::Session<moqt::QUIC>>,
        sender: tokio::sync::mpsc::Sender<String>,
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
                        sender
                            .send(publish_namespace_handler.track_namespace)
                            .await
                            .unwrap_or_else(|e| {
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
                        let _ = subscribe_handler
                            .ok(0, 1000000, moqt::ContentExists::False)
                            .await;
                    }
                    moqt::SessionEvent::ProtocolViolation() => {
                        tracing::info!("Received: ProtocolViolation");
                    }
                };
            }
        })
    }
}
