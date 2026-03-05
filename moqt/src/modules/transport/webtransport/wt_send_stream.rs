use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::transport_send_stream::TransportSendStream;

#[derive(Debug)]
#[allow(dead_code)]
pub struct WtSendStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) send_stream: wtransport::SendStream,
}

#[async_trait]
impl TransportSendStream for WtSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> anyhow::Result<()> {
        Ok(self.send_stream.write_all(buffer).await?)
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        Ok(self.send_stream.finish().await?)
    }
}
