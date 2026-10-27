use std::{pin::Pin, task::Poll};

use tokio::io::{AsyncRead, ReadBuf};

use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct WtReceiveStream {
    pub(crate) recv_stream: web_transport_quinn::RecvStream,
}

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

    fn stop(&mut self, error_code: u64) {
        let Ok(error_code) = u32::try_from(error_code) else {
            tracing::debug!(error_code, "STOP_SENDING code out of range");
            return;
        };
        // Fails only for a stream that already ended.
        let _ = self.recv_stream.stop(error_code);
    }
}
