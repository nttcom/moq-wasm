use std::{pin::Pin, task::Poll};

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};

use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
#[allow(dead_code)]
pub struct WtReceiveStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) recv_stream: web_transport_quinn::RecvStream,
}

#[async_trait]
impl TransportReceiveStream for WtReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut BytesMut,
    ) -> Poll<Result<usize, ReadError>> {
        let mut read_buf = ReadBuf::new(buf);
        let filled_before = read_buf.filled().len();
        match Pin::new(&mut self.recv_stream).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let size = read_buf.filled().len() - filled_before;
                Poll::Ready(Ok(size))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(match e.kind() {
                std::io::ErrorKind::ConnectionReset => ReadError::Reset,
                std::io::ErrorKind::ConnectionAborted => ReadError::ConnectionLost,
                std::io::ErrorKind::UnexpectedEof => ReadError::Closed,
                _ => todo!("handle other web-transport-quinn read errors: {:?}", e),
            })),
            Poll::Pending => Poll::Pending,
        }
    }
}
