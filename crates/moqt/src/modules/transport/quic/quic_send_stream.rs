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
}

impl QUICSendStream {
    pub(crate) fn new(send_stream: quinn::SendStream) -> Self {
        Self {
            send_stream,
            stopped: None,
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
