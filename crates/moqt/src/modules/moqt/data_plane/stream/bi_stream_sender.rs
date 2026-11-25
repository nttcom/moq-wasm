use bytes::BytesMut;

use crate::{
    modules::{
        executor,
        moqt::{
            control_plane::control_messages::control_message_type::ControlMessageType,
            data_plane::stream::stream_sender::StreamSender,
        },
        transport::{
            transport_connection::BoxedSendStream, transport_send_stream::TransportSendError,
        },
    },
    wire::encode_control_message,
};

#[derive(Debug)]
pub(crate) struct BiStreamSender {
    stream_sender: StreamSender,
}

impl BiStreamSender {
    pub(crate) fn new(stream_sender: BoxedSendStream) -> Self {
        Self {
            stream_sender: StreamSender::new(stream_sender),
        }
    }

    pub(crate) async fn send(
        &self,
        message_type: ControlMessageType,
        bytes: BytesMut,
    ) -> Result<(), TransportSendError> {
        self.queue(message_type, bytes).await?.write().await
    }

    /// Frames the message and waits for the stream, writing nothing: a
    /// caller dropped here leaves the stream untouched.
    pub(crate) async fn queue(
        &self,
        message_type: ControlMessageType,
        bytes: BytesMut,
    ) -> Result<QueuedMessage, TransportSendError> {
        let framed = encode_control_message(message_type, bytes)
            .map_err(|source| TransportSendError::Unencodable { source })?;
        Ok(QueuedMessage {
            stream: self.stream_sender.lock_owned().await,
            framed,
        })
    }

    #[cfg(test)]
    pub(crate) async fn hold(&self) -> tokio::sync::MutexGuard<'_, BoxedSendStream> {
        self.stream_sender.lock().await
    }
}

/// A control message holding the stream, next to be written on it.
pub(crate) struct QueuedMessage {
    stream: tokio::sync::OwnedMutexGuard<BoxedSendStream>,
    framed: BytesMut,
}

impl QueuedMessage {
    /// Writes the message in a task of its own, so the write completes even
    /// if the returned future is dropped: a partial message would make the
    /// peer parse the next one as its continuation.
    pub(crate) fn write(self) -> impl Future<Output = Result<(), TransportSendError>> {
        let Self { mut stream, framed } = self;
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        executor::spawn("Control Message Write", async move {
            let _ = result_sender.send(stream.send(&framed).await);
        });
        async move {
            result_receiver
                .await
                .unwrap_or(Err(TransportSendError::ClosedStream))
        }
    }
}
