use std::task::Poll;

use crate::modules::transport::{
    transport_connection::BoxedSendStream, transport_send_stream::TransportSendError,
};
use bytes::BytesMut;

#[derive(Debug)]
pub struct StreamSender {
    send_stream: tokio::sync::Mutex<BoxedSendStream>,
}

impl StreamSender {
    pub(crate) fn new(send_stream: BoxedSendStream) -> Self {
        Self {
            send_stream: tokio::sync::Mutex::new(send_stream),
        }
    }

    pub async fn send(&self, bytes: &BytesMut) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.send(bytes).await
    }

    pub async fn set_priority(&self, priority: i32) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.set_priority(priority)
    }

    pub async fn close(&self) -> Result<(), TransportSendError> {
        self.send_stream.lock().await.close()
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
