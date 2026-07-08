use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

#[derive(Debug)]
#[allow(dead_code)]
pub struct WtSendStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
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
        web_transport_quinn::WriteError::InvalidStopped(code) => {
            TransportSendError::InvalidStopped {
                code: code.into_inner(),
            }
        }
        web_transport_quinn::WriteError::SessionError(error) => TransportSendError::SessionError {
            reason: error.to_string(),
        },
        web_transport_quinn::WriteError::ClosedStream => TransportSendError::ClosedStream,
    }
}
