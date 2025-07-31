use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;
use quinn::{self, RecvStream};
use std::sync::Arc;
use tokio::sync::Mutex;

#[automock]
#[async_trait]
pub(crate) trait BiStreamTrait {
    async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()>;
}

pub(crate) struct QuicBiStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) recv_stream: quinn::RecvStream,
    pub(crate) shared_send_stream: Arc<Mutex<quinn::SendStream>>,
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
}

impl QuicBiStream {
    pub(super) fn new(
        stable_id: usize,
        stream_id: u64,
        recv_stream: RecvStream,
        send_stream: Arc<Mutex<quinn::SendStream>>,
    ) -> Self {
        Self {
            stable_id,
            stream_id,
            recv_stream,
            shared_send_stream: send_stream,
        }
    }
}
