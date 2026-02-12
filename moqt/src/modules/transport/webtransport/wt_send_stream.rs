use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::transport_send_stream::TransportSendStream;

#[derive(Debug)]
pub struct WtSendStream;

#[async_trait]
impl TransportSendStream for WtSendStream {
    async fn send(&mut self, _buffer: &BytesMut) -> anyhow::Result<()> {
        todo!("WebTransport send not yet implemented")
    }
}
