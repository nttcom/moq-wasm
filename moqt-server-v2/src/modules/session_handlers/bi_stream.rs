use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;
use quinn::{self, RecvStream};
use std::sync::{Arc, Mutex, Weak};
use tokio::task;

#[automock]
#[async_trait]
pub(crate) trait BiStreamTrait: Send + Sync {
    async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()>;
    async fn start_receive(&mut self, event_sender: std::sync::mpsc::Sender::<ReceiveEventType>);
}

#[derive(Clone)]
pub(crate) enum ReceiveEventType {
    OnMessageReceived{stream_id: u64, buffer: BytesMut},
    OnError{stream_id: u64, message: String}
}

pub(crate) struct QuicBiStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) recv_stream: Arc<tokio::sync::Mutex<RecvStream>>,
    pub(crate) shared_send_stream: Arc<tokio::sync::Mutex<quinn::SendStream>>,
}

#[async_trait]
impl BiStreamTrait for QuicBiStream {

    async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()> {
        Ok(self
            .shared_send_stream
            .lock()
            .await
            .write_all(&buffer)
            .await?)
    }

    async fn start_receive(&mut self, event_sender: std::sync::mpsc::Sender::<ReceiveEventType>) {
        let stream_id = self.stream_id;
        let name = format!("Receiver id {} thread", stream_id);
        let _recv_stream = self.recv_stream.clone();
        task::Builder::new().name(&name).spawn(async move {
            loop {
                let message = match _recv_stream.lock().await.read_to_end(1024).await {
                    Ok(buffer) => {
                        let mut bytes = BytesMut::with_capacity(buffer.len());
                        bytes.extend_from_slice(&buffer);
                        ReceiveEventType::OnMessageReceived{stream_id, buffer: bytes}
                    },
                    Err(e) => ReceiveEventType::OnError{stream_id, message: e.to_string()}
                };
                let result = event_sender.send(message.clone());
                if result.is_err() || matches!(message, ReceiveEventType::OnError {..}) {
                    tracing::warn!("Receiver has already been released.");
                    break;
                }
            }
        });
    }
}

impl QuicBiStream {
    pub(super) fn new(
        stable_id: usize,
        stream_id: u64,
        recv_stream: RecvStream,
        send_stream: quinn::SendStream,
    ) -> Self {
        Self {
            stable_id,
            stream_id,
            recv_stream: Arc::new(tokio::sync::Mutex::new(recv_stream)),
            shared_send_stream: Arc::new(tokio::sync::Mutex::new(send_stream))
        }
    }
}
