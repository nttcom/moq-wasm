use std::task::Poll;

use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self, RecvStream};

use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct QUICReceiveStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) recv_stream: RecvStream,
}

#[async_trait]
impl TransportReceiveStream for QUICReceiveStream {
    async fn receive(&mut self, buffer: &mut BytesMut) -> anyhow::Result<Option<usize>> {
        // `read_chunk` that reads received data from QUIC buffer directly
        // should be used when it comes to performance.
        // However `wtransport` does not have api that is equivalent to `read_chunk`.
        let result = self.recv_stream.read(buffer).await?;
        Ok(result)
    }

    fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut BytesMut,
    ) -> Poll<Result<usize, ReadError>> {
        let result = self.recv_stream.poll_read(cx, buf).map_err(|e| match e {
            quinn::ReadError::ClosedStream => ReadError::Closed,
            quinn::ReadError::Reset(_) => ReadError::Reset,
            quinn::ReadError::ConnectionLost(_) => ReadError::ConnectionLost,
            _ => todo!("handle other read errors: {:?}", e),
        });
        result
    }
}
