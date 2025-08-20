use anyhow::bail;
use async_trait::async_trait;
use bytes::BytesMut;
use mockall::automock;
use quinn::{self, RecvStream};
use std::sync::Arc;

#[automock]
#[async_trait]
pub(crate) trait BiStreamTrait: Send + Sync + 'static {
    fn get_stream_id(&self) -> u64;
    async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()>;
    async fn receive(&mut self) -> anyhow::Result<BytesMut>;
}

pub(crate) struct QuicBiStream {
    pub(crate) stable_id: usize,
    stream_id: u64,
    pub(crate) recv_stream: RecvStream,
    pub(crate) shared_send_stream: Arc<tokio::sync::Mutex<quinn::SendStream>>,
}

#[async_trait]
impl BiStreamTrait for QuicBiStream {

    fn get_stream_id(&self) -> u64 {
        self.stream_id
    }

    async fn send(&self, buffer: &BytesMut) -> anyhow::Result<()> {
        Ok(self
            .shared_send_stream
            .lock()
            .await
            .write_all(buffer)
            .await?)
    }

    async fn receive(&mut self) -> anyhow::Result<BytesMut> {
        match self.recv_stream.read_to_end(1024).await {
            Ok(data) => {
                let mut bytes = BytesMut::with_capacity(1024);
                bytes.extend_from_slice(&data);
                Ok(bytes)
            },
            Err(e) => {
                bail!("{}", e)
            }
        }
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
            recv_stream,
            shared_send_stream: Arc::new(tokio::sync::Mutex::new(send_stream))
        }
    }
}
