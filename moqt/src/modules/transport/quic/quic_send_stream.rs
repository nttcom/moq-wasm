use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self};

use crate::modules::transport::transport_send_stream::TransportSendStream;

#[derive(Debug)]
#[allow(dead_code)]
pub struct QUICSendStream {
    pub(crate) send_stream: quinn::SendStream,
}

#[async_trait]
impl TransportSendStream for QUICSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> anyhow::Result<()> {
        Ok(self.send_stream.write_all(buffer).await?)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        Ok(self.send_stream.finish()?)
    }
}
