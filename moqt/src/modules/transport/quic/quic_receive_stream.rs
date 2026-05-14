use std::task::Poll;

use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self, RecvStream};

use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct QUICReceiveStream {
    pub(crate) recv_stream: RecvStream,
}

#[async_trait]
impl TransportReceiveStream for QUICReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut BytesMut,
    ) -> Poll<Result<usize, ReadError>> {
        self.recv_stream.poll_read(cx, buf).map_err(|e| match e {
            quinn::ReadError::ClosedStream => ReadError::Closed,
            quinn::ReadError::Reset(_) => ReadError::Reset,
            quinn::ReadError::ConnectionLost(_) => ReadError::ConnectionLost,
            _ => todo!("handle other read errors: {:?}", e),
        })
    }
}
