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

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        let error_code =
            u32::try_from(error_code).map_err(|source| TransportSendError::Transport {
                source: source.into(),
            })?;
        self.send_stream
            .reset(error_code)
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
