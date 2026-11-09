use std::task::Poll;

use quinn::{self, RecvStream};
use tokio::io::ReadBuf;

use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct QUICReceiveStream {
    pub(crate) recv_stream: RecvStream,
}

impl TransportReceiveStream for QUICReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), ReadError>> {
        self.recv_stream
            .poll_read_buf(cx, buf)
            .map_err(|e| match e {
                quinn::ReadError::ClosedStream => ReadError::Closed,
                quinn::ReadError::Reset(_) => ReadError::Reset,
                quinn::ReadError::ConnectionLost(_) => ReadError::ConnectionLost,
                _ => todo!("handle other read errors: {:?}", e),
            })
    }

    fn stop(&mut self, error_code: u64) {
        let Ok(error_code) = quinn::VarInt::try_from(error_code) else {
            tracing::debug!(error_code, "STOP_SENDING code out of range");
            return;
        };
        // Fails only for a stream that already ended.
        let _ = self.recv_stream.stop(error_code);
    }
}
