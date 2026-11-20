use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::BytesMut;
use web_transport_quinn::generic::poll::SendStream as _;

use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

#[derive(Debug)]
pub struct WtSendStream {
    send_stream: web_transport_quinn::SendStream,
    /// Set once `close` or `reset` ran, so `Drop` does not reset a stream
    /// already finished: quinn still accepts a reset until the FIN is
    /// acknowledged.
    ended: bool,
}

impl WtSendStream {
    pub(crate) fn new(send_stream: web_transport_quinn::SendStream) -> Self {
        Self {
            send_stream,
            ended: false,
        }
    }
}

#[async_trait]
impl TransportSendStream for WtSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        self.send_stream
            .write_all(buffer)
            .await
            .map_err(webtransport_write_error_to_transport_send_error)
    }

    fn close(&mut self) -> Result<(), TransportSendError> {
        self.ended = true;
        self.send_stream
            .finish()
            .map_err(|_| TransportSendError::ClosedStream)
    }

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        let error_code =
            u32::try_from(error_code).map_err(|source| TransportSendError::Transport {
                source: source.into(),
            })?;
        self.ended = true;
        self.send_stream
            .reset(error_code)
            .map_err(|_| TransportSendError::ClosedStream)
    }

    fn set_priority(&mut self, priority: i32) -> Result<(), TransportSendError> {
        self.send_stream
            .set_priority(priority)
            .map_err(|_| TransportSendError::ClosedStream)
    }

    /// `poll_closed` keeps the wait it starts inside the stream.
    fn poll_stopped(&mut self, cx: &mut Context<'_>) -> Poll<Option<u64>> {
        self.send_stream.poll_closed(cx).map(|closed| match closed {
            Err(web_transport_quinn::WriteError::Stopped(code)) => Some(u64::from(code)),
            Err(web_transport_quinn::WriteError::InvalidStopped(code)) => Some(code.into_inner()),
            Ok(()) | Err(_) => None,
        })
    }
}

/// A stream dropped before `close` or `reset` is reset with code 0: the
/// quinn stream underneath would finish it, making a partial subgroup look
/// complete to the peer (draft-14 §10.4.3).
impl Drop for WtSendStream {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.send_stream.reset(0);
        }
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
    use crate::modules::{
        test_support::{WRITTEN, within, written_uni_stream},
        transport::{qmux::in_memory::read_to_end, read_error::ReadError},
    };

    #[test]
    fn stop_sending_outside_webtransport_code_range_is_stopped_by_peer() {
        // Arrange
        let error = web_transport_quinn::WriteError::InvalidStopped(quinn::VarInt::from_u32(0));

        // Act
        let mapped = webtransport_write_error_to_transport_send_error(error);

        // Assert
        assert!(matches!(mapped, TransportSendError::Stopped { code: 0 }));
    }

    #[tokio::test]
    async fn a_stream_dropped_before_close_is_reset() {
        // Arrange
        let mut stream = written_uni_stream("wt-drop-reset", "https", "/moq").await;

        // Act
        drop(stream.sender);

        // Assert: the peer does not take the partial data for a complete stream
        let read = within(read_to_end(&mut stream.receiver)).await;
        assert_eq!(read, Err(ReadError::Reset));
    }

    #[tokio::test]
    async fn a_stream_dropped_after_close_ends_with_fin() {
        // Arrange
        let mut stream = written_uni_stream("wt-close-fin", "https", "/moq").await;

        // Act
        stream.sender.close().unwrap();
        drop(stream.sender);

        // Assert
        let read = within(read_to_end(&mut stream.receiver)).await;
        assert_eq!(read, Ok(WRITTEN.to_vec()));
    }
}
