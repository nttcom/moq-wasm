use std::{pin::Pin, task::Poll};

use async_trait::async_trait;
use tokio::io::{AsyncRead, ReadBuf};

use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct WtReceiveStream {
    pub(crate) recv_stream: web_transport_quinn::RecvStream,
}

#[async_trait]
impl TransportReceiveStream for WtReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), ReadError>> {
        Pin::new(&mut self.recv_stream)
            .poll_read(cx, buf)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::ConnectionReset => ReadError::Reset,
                std::io::ErrorKind::ConnectionAborted => ReadError::ConnectionLost,
                std::io::ErrorKind::NotConnected => ReadError::Closed,
                std::io::ErrorKind::UnexpectedEof => ReadError::Closed,
                _ => todo!("handle other web-transport-quinn read errors: {:?}", e),
            })
    }
}
