use bytes::BytesMut;

use crate::{
    modules::{
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
        self.stream_sender
            .send(&encode_control_message(message_type, bytes))
            .await
    }
}
