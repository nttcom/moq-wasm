use std::{sync::Arc, task::Poll};

use crate::modules::{
    executor,
    moqt::data_plane::stream::unacknowledged_streams::{SharedSendStream, UnacknowledgedStreams},
    transport::{transport_connection::BoxedSendStream, transport_send_stream::TransportSendError},
};
use bytes::BytesMut;

#[derive(Debug)]
pub struct StreamSender {
    send_stream: SharedSendStream,
    /// `Some` for a subgroup or FETCH stream; `close` hands the stream to it
    /// after sending the FIN.
    unacknowledged: Option<UnacknowledgedStreams>,
}

impl StreamSender {
    // `Arc`, not `Rc`: native builds share the stream across threads, while on
    // wasm32 `BoxedSendStream` is not `Send`.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) fn new(send_stream: BoxedSendStream) -> Self {
        Self {
            send_stream: Arc::new(tokio::sync::Mutex::new(send_stream)),
            unacknowledged: None,
        }
    }

    /// `close` hands the stream, after its FIN, to `unacknowledged`, which
    /// `Session::close` waits on.
    pub(crate) fn held_until_acknowledged(
        send_stream: BoxedSendStream,
        unacknowledged: &UnacknowledgedStreams,
    ) -> Self {
        Self {
            unacknowledged: Some(unacknowledged.clone()),
            ..Self::new(send_stream)
        }
    }

    #[cfg(test)]
    pub(crate) async fn lock(&self) -> tokio::sync::MutexGuard<'_, BoxedSendStream> {
        self.send_stream.lock().await
    }

    /// Waits for the stream, writing nothing: a caller dropped here leaves
    /// the stream untouched.
    pub(crate) async fn queue(&self, bytes: BytesMut) -> QueuedMessage {
        QueuedMessage {
            stream: self.send_stream.clone().lock_owned().await,
            bytes,
        }
    }

    pub async fn send(&self, bytes: &BytesMut) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.send(bytes).await
    }

    pub async fn set_priority(&self, priority: i32) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.set_priority(priority)
    }

    pub async fn close(&self) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.close()?;
        if let Some(unacknowledged) = &self.unacknowledged {
            unacknowledged.hold(self.send_stream.clone());
        }
        Ok(())
    }

    pub async fn reset(&self, error_code: u64) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.reset(error_code).await
    }

    /// The peer's STOP_SENDING code once it stops the stream, `None` once it
    /// no longer can (see `TransportSendStream::poll_stopped`). The stream
    /// is held only while polled, so sends go on while this waits.
    pub(crate) async fn stopped(&self) -> Option<u64> {
        loop {
            let mut stream = self.send_stream.lock().await;
            let polled = std::future::poll_fn(|cx| Poll::Ready(stream.poll_stopped(cx))).await;
            drop(stream);
            if let Poll::Ready(code) = polled {
                return code;
            }
            // The poll registered this task's waker; wait for it.
            let mut woken = false;
            std::future::poll_fn(|_| {
                if std::mem::replace(&mut woken, true) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        }
    }
}

/// A message holding the stream, next to be written on it.
pub(crate) struct QueuedMessage {
    stream: tokio::sync::OwnedMutexGuard<BoxedSendStream>,
    bytes: BytesMut,
}

impl QueuedMessage {
    /// Writes the message in a task of its own, so the write completes even
    /// if the returned future is dropped: a partial message would make the
    /// peer parse the next one as its continuation.
    pub(crate) fn write(self) -> impl Future<Output = Result<(), TransportSendError>> {
        let Self { mut stream, bytes } = self;
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        executor::spawn("Control Message Write", async move {
            let _ = result_sender.send(stream.send(&bytes).await);
        });
        async move {
            result_receiver
                .await
                .unwrap_or(Err(TransportSendError::ClosedStream))
        }
    }
}
