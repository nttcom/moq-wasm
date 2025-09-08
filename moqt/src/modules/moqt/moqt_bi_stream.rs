use bytes::BytesMut;
use std::sync::Arc;

use crate::modules::transport::transport_bi_stream::TransportBiStream;

#[derive(Clone)]
pub(crate) enum ReceiveEvent {
    Message(Vec<u8>),
    Error(),
}

pub(crate) struct MOQTBiStream {
    transport_bi_stream: Arc<tokio::sync::Mutex<dyn TransportBiStream>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl MOQTBiStream {
    const RECEIVE_BYTES_CAPACITY: usize = 1024;

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
                let mut total_message = vec![];
                loop {
                    let mut bytes = vec![0u8; Self::RECEIVE_BYTES_CAPACITY];
                    let message = transport_bi_stream.lock().await.receive(&mut bytes).await;
                    if let Err(e) = message {
                        tracing::error!("failed to receive message: {:?}", e);
                        Self::disptach_receive_event(&sender, ReceiveEvent::Error());
                        break;
                    }
                    if let Some(size) = message.unwrap() {
                        tracing::debug!("Size {} message has been received", size);
                        total_message.extend_from_slice(&bytes[..size]);
                        if size == Self::RECEIVE_BYTES_CAPACITY {
                            tracing::debug!("Retry...");
                        } else {
                            tracing::debug!("message length: {}", total_message.len());
                            Self::disptach_receive_event(
                                &sender,
                                ReceiveEvent::Message(total_message.clone()),
                            );
                        }
                    } else {
                        tracing::debug!("message length: {}", total_message.len());
                        Self::disptach_receive_event(
                            &sender,
                            ReceiveEvent::Message(total_message.clone()),
                        );
                        total_message.clear();
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
                tracing::debug!("Received message has been sent.");
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
