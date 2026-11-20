use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::BytesMut;
use quinn::{self};

use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

type Stopped = Pin<Box<dyn Future<Output = Option<u64>> + Send + Sync>>;

pub struct QUICSendStream {
    send_stream: quinn::SendStream,
    /// The wait `poll_stopped` started, kept until it resolves so that it
    /// stays registered.
    stopped: Option<Stopped>,
    /// Set once `close` or `reset` ran, so `Drop` does not reset a stream
    /// already finished: quinn still accepts a reset until the FIN is
    /// acknowledged.
    ended: bool,
}

impl QUICSendStream {
    pub(crate) fn new(send_stream: quinn::SendStream) -> Self {
        Self {
            send_stream,
            stopped: None,
            ended: false,
        }
    }
}

impl fmt::Debug for QUICSendStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QUICSendStream")
            .field("send_stream", &self.send_stream)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl TransportSendStream for QUICSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        self.send_stream
            .write_all(buffer)
            .await
            .map_err(quic_write_error_to_transport_send_error)
    }

    fn close(&mut self) -> Result<(), TransportSendError> {
        self.ended = true;
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

    fn poll_stopped(&mut self, cx: &mut Context<'_>) -> Poll<Option<u64>> {
        let stopped = self.stopped.get_or_insert_with(|| {
            let stopped = self.send_stream.stopped();
            Box::pin(async move { stopped.await.ok().flatten().map(quinn::VarInt::into_inner) })
        });
        let polled = stopped.as_mut().poll(cx);
        if polled.is_ready() {
            // A finished future cannot be polled again; the next poll asks anew.
            self.stopped = None;
        }
        polled
    }
}

/// A stream dropped before `close` or `reset` is reset with code 0: quinn
/// would finish it, making a partial subgroup look complete to the peer
/// (draft-14 §10.4.3).
impl Drop for QUICSendStream {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.send_stream.reset(quinn::VarInt::from_u32(0));
        }
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

#[cfg(test)]
mod tests {
    use crate::modules::{
        test_support::{WRITTEN, within, written_uni_stream},
        transport::{qmux::in_memory::read_to_end, read_error::ReadError},
    };

    #[tokio::test]
    async fn a_stream_dropped_before_close_is_reset() {
        // Arrange
        let mut stream = written_uni_stream("quic-drop-reset", "moqt", "").await;

        // Act
        drop(stream.sender);

        // Assert: the peer does not take the partial data for a complete stream
        let read = within(read_to_end(&mut stream.receiver)).await;
        assert_eq!(read, Err(ReadError::Reset));
    }

    #[tokio::test]
    async fn a_stream_dropped_after_close_ends_with_fin() {
        // Arrange
        let mut stream = written_uni_stream("quic-close-fin", "moqt", "").await;

        // Act
        stream.sender.close().unwrap();
        drop(stream.sender);

        // Assert
        let read = within(read_to_end(&mut stream.receiver)).await;
        assert_eq!(read, Ok(WRITTEN.to_vec()));
    }
}
