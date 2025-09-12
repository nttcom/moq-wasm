use async_trait::async_trait;
use quinn::{self, RecvStream};

use crate::modules::transport::transport_receive_stream::TransportReceiveStream;

#[derive(Debug)]

pub struct QUICReceiveStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) recv_stream: RecvStream,
}

#[async_trait]
impl TransportReceiveStream for QUICReceiveStream {
    async fn receive(&mut self, buffer: &mut Vec<u8>) -> anyhow::Result<Option<usize>> {
        // `read_chunk` that reads received data from QUIC buffer directly
        // should be used when it comes to performance.
        // However `wtransport` does not have api that is equivalent to `read_chunk`.
        let result = self.recv_stream.read(buffer).await?;
        Ok(result)
    }
}
