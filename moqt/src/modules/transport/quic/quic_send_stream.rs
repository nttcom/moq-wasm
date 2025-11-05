use anyhow::bail;
use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self};

use crate::modules::transport::transport_send_stream::TransportSendStream;

#[derive(Debug)]
pub struct QUICSendStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) send_stream: quinn::SendStream,
}

#[async_trait]
impl TransportSendStream for QUICSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> anyhow::Result<()> {
        Ok(self.send_stream.write_all(buffer).await?)
    }
}
