use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self};

use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

#[derive(Debug)]
pub struct QUICSendStream {
    pub(crate) send_stream: quinn::SendStream,
}

#[async_trait]
impl TransportSendStream for QUICSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        self.send_stream
            .write_all(buffer)
            .await
            .map_err(quic_write_error_to_transport_send_error)
    }

    async fn close(&mut self) -> Result<(), TransportSendError> {
        self.send_stream
            .finish()
            .map_err(|_| TransportSendError::ClosedStream)
    }

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        let error_code = quinn::VarInt::try_from(error_code).map_err(|source| {
            TransportSendError::Transport {
                source: source.into(),
            }
        })?;
        self.send_stream
            .reset(error_code)
            .map_err(|_| TransportSendError::ClosedStream)
    }
}

fn quic_write_error_to_transport_send_error(error: quinn::WriteError) -> TransportSendError {
    match error {
        quinn::WriteError::Stopped(code) => TransportSendError::Stopped {
            code: code.into_inner(),
        },
        quinn::WriteError::ConnectionLost(error) => TransportSendError::ConnectionLost {
            reason: error.to_string(),
        },
        quinn::WriteError::ClosedStream => TransportSendError::ClosedStream,
        quinn::WriteError::ZeroRttRejected => TransportSendError::ZeroRttRejected,
    }
}
