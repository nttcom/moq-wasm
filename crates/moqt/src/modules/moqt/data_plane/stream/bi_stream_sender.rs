use bytes::BytesMut;

use crate::{
    modules::{
        moqt::{
            control_plane::control_messages::control_message_type::ControlMessageType,
            data_plane::stream::stream_sender::{QueuedMessage, StreamSender},
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
        Ok(self.stream_sender.queue(framed).await)
    }

    #[cfg(test)]
    pub(crate) async fn hold(&self) -> tokio::sync::MutexGuard<'_, BoxedSendStream> {
        self.stream_sender.lock().await
    }
}
