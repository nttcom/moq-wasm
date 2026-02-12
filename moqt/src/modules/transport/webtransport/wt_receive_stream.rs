use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::transport_receive_stream::TransportReceiveStream;

#[derive(Debug)]
pub struct WtReceiveStream;

#[async_trait]
impl TransportReceiveStream for WtReceiveStream {
    async fn receive(&mut self, _buffer: &mut BytesMut) -> anyhow::Result<Option<usize>> {
        todo!("WebTransport receive not yet implemented")
    }
}
