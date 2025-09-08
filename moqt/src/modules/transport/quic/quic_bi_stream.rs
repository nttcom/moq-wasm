use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self, RecvStream};
use std::sync::Arc;

use crate::modules::transport::transport_bi_stream::TransportBiStream;

pub(crate) struct QUICBiStream {
    pub(crate) stable_id: usize,
    stream_id: u64,
    pub(crate) recv_stream: RecvStream,
    pub(crate) shared_send_stream: Arc<tokio::sync::Mutex<quinn::SendStream>>,
}

#[async_trait]
impl TransportBiStream for QUICBiStream {
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

    async fn receive(&mut self, buffer: &mut Vec<u8>) -> anyhow::Result<Option<usize>> {
        // `read_chunk` that reads received data from QUIC buffer directly
        // should be used when it comes to performance.
        // However `wtransport` does not have api that is equivalent to `read_chunk`.
        let result = self.recv_stream.read(buffer).await?;
        Ok(result)
    }
}

impl QUICBiStream {
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
            shared_send_stream: Arc::new(tokio::sync::Mutex::new(send_stream)),
        }
    }
}
