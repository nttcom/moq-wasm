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
            .map_err(|e| io_error_to_read_error(&e))
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

fn io_error_to_read_error(error: &std::io::Error) -> ReadError {
    match error.kind() {
        std::io::ErrorKind::ConnectionReset => ReadError::Reset(reset_code(error)),
        std::io::ErrorKind::ConnectionAborted => ReadError::ConnectionLost,
        std::io::ErrorKind::NotConnected
            if matches!(
                error
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<quinn::ReadError>()),
                Some(quinn::ReadError::ConnectionLost(_))
            ) =>
        {
            ReadError::ConnectionLost
        }
        std::io::ErrorKind::NotConnected => ReadError::Closed,
        std::io::ErrorKind::UnexpectedEof => ReadError::Closed,
        _ => todo!("handle other web-transport-quinn read errors: {:?}", error),
    }
}

/// The application code of a RESET_STREAM, which WebTransport carries in
/// the HTTP/3 error space. A code outside the WebTransport range was not sent
/// by the peer's application, so it reads as 0, INTERNAL_ERROR in every draft.
fn reset_code(error: &std::io::Error) -> u64 {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<quinn::ReadError>())
    {
        Some(quinn::ReadError::Reset(code)) => {
            web_transport_quinn::proto::error_from_http3(code.into_inner()).map_or(0, u64::from)
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_connection_is_a_lost_connection() {
        // Arrange: quinn's io::Error for a read on a connection the session closed
        let error = std::io::Error::from(quinn::ReadError::ConnectionLost(
            quinn::ConnectionError::LocallyClosed,
        ));

        // Act
        let mapped = io_error_to_read_error(&error);

        // Assert
        assert_eq!(mapped, ReadError::ConnectionLost);
    }

    #[test]
    fn a_reset_carries_the_webtransport_code() {
        // Arrange
        let http3_code = web_transport_quinn::proto::error_to_http3(2);
        let error = std::io::Error::from(quinn::ReadError::Reset(
            quinn::VarInt::from_u64(http3_code).unwrap(),
        ));

        // Act
        let mapped = io_error_to_read_error(&error);

        // Assert
        assert_eq!(mapped, ReadError::Reset(2));
    }

    #[test]
    fn a_closed_stream_is_closed() {
        // Arrange
        let error = std::io::Error::from(quinn::ReadError::ClosedStream);

        // Act
        let mapped = io_error_to_read_error(&error);

        // Assert
        assert_eq!(mapped, ReadError::Closed);
    }
}
