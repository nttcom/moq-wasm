use bytes::BytesMut;
use std::sync::Arc;

use crate::modules::transport::transport_bi_stream::TransportBiStream;

#[derive(Clone)]
pub(crate) enum ReceiveEvent {
    Message(BytesMut),
    Error(),
}

pub(crate) struct MOQTBiStream {
    transport_bi_stream: Arc<tokio::sync::Mutex<dyn TransportBiStream>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl MOQTBiStream {
    const RECEIVE_BYTES_CAPACITY: usize = 8192;

    pub(crate) fn new(
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
        transport_bi_stream: Arc<tokio::sync::Mutex<dyn TransportBiStream>>,
    ) -> Self {
        Self {
            join_handle: Self::create_join_handle(transport_bi_stream.clone(), sender),
            transport_bi_stream,
        }
    }

    fn create_join_handle(
        transport_bi_stream: Arc<tokio::sync::Mutex<dyn TransportBiStream>>,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("MOQTBiStream")
            .spawn(async move {
                let mut total_message = BytesMut::new();
                loop {
                    let mut bytes = BytesMut::with_capacity(Self::RECEIVE_BYTES_CAPACITY);
                    let message = transport_bi_stream.lock().await.receive(&mut bytes).await;
                    match message {
                        Ok(o) => {
                            if let Some(n) = o {
                                tracing::info!("Retry to receive message.");
                                total_message.extend_from_slice(&bytes[..n]);
                            } else {
                                Self::disptach_receive_event(
                                    &sender,
                                    ReceiveEvent::Message(total_message.clone()),
                                );
                                total_message.clear();
                            }
                        }
                        Err(_) => {
                            Self::disptach_receive_event(&sender, ReceiveEvent::Error());
                            break;
                        }
                    }
                }
            })
            .unwrap()
    }

    fn disptach_receive_event(
        sender: &tokio::sync::broadcast::Sender<ReceiveEvent>,
        receive_event: ReceiveEvent,
    ) {
        loop {
            if sender.send(receive_event.clone()).is_ok() {
                tracing::info!("Received message has been sent.");
                break;
            } else {
                tracing::warn!("Sending message failed. Retry.");
                continue;
            }
        }
    }

    pub(crate) async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()> {
        self.transport_bi_stream.lock().await.send(buffer).await
    }
}

impl Drop for MOQTBiStream {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}