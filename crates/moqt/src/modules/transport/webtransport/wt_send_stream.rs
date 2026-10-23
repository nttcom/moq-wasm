use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

#[derive(Debug)]
pub struct WtSendStream {
    pub(crate) send_stream: web_transport_quinn::SendStream,
}

#[async_trait]
impl TransportSendStream for WtSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        self.send_stream
            .write_all(buffer)
            .await
            .map_err(webtransport_write_error_to_transport_send_error)
    }

    async fn close(&mut self) -> Result<(), TransportSendError> {
        self.send_stream
            .finish()
            .map_err(|_| TransportSendError::ClosedStream)
    }

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        let error_code =
            u32::try_from(error_code).map_err(|source| TransportSendError::Transport {
                source: source.into(),
            })?;
        self.send_stream
            .reset(error_code)
            .map_err(|_| TransportSendError::ClosedStream)
    }

    fn set_priority(&mut self, priority: i32) -> Result<(), TransportSendError> {
        self.send_stream
            .set_priority(priority)
            .map_err(|_| TransportSendError::ClosedStream)
    }
}

fn webtransport_write_error_to_transport_send_error(
    error: web_transport_quinn::WriteError,
) -> TransportSendError {
    match error {
        web_transport_quinn::WriteError::Stopped(code) => TransportSendError::Stopped {
            code: u64::from(code),
        },
        // A peer's quinn RecvStream sends STOP_SENDING(0) without the WebTransport code mapping
        // when dropped, so a code outside the WebTransport range is still a STOP_SENDING.
        web_transport_quinn::WriteError::InvalidStopped(code) => TransportSendError::Stopped {
            code: code.into_inner(),
        },
        web_transport_quinn::WriteError::SessionError(error) => TransportSendError::SessionError {
            reason: error.to_string(),
        },
        web_transport_quinn::WriteError::ClosedStream => TransportSendError::ClosedStream,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_sending_outside_webtransport_code_range_is_stopped_by_peer() {
        // Arrange
        let error = web_transport_quinn::WriteError::InvalidStopped(quinn::VarInt::from_u32(0));

        // Act
        let mapped = webtransport_write_error_to_transport_send_error(error);

        // Assert
        assert!(matches!(mapped, TransportSendError::Stopped { code: 0 }));
    }
}
