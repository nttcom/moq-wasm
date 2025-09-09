use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self};

use crate::modules::transport::transport_send_stream::TransportSendStream;

pub(crate) struct QUICSendStream {
    pub(crate) stable_id: usize,
    stream_id: u64,
    send_stream: quinn::SendStream,
}

#[async_trait]
impl TransportSendStream for QUICSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> anyhow::Result<()> {
        Ok(self.send_stream.write_all(buffer).await?)
    }
}

impl QUICSendStream {
    pub(super) fn new(stable_id: usize, stream_id: u64, send_stream: quinn::SendStream) -> Self {
        Self {
            stable_id,
            stream_id,
            send_stream,
        }
    }
}
