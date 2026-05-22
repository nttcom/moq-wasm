use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::{
        moqt::control_plane::control_messages::control_message_type::ControlMessageType,
        transport::transport_send_stream::TransportSendStream,
    },
    wire::encode_control_message,
};

#[derive(Debug)]
pub(crate) struct BiStreamSender<T: TransportProtocol> {
    stream_sender: tokio::sync::Mutex<T::SendStream>,
}

impl<T: TransportProtocol> BiStreamSender<T> {
    pub(crate) fn new(stream_sender: T::SendStream) -> Self {
        Self {
            stream_sender: tokio::sync::Mutex::new(stream_sender),
        }
    }

    pub(crate) async fn send(
        &self,
        message_type: ControlMessageType,
        bytes: BytesMut,
    ) -> anyhow::Result<()> {
        let message_bytes = encode_control_message(message_type, bytes);
        let mut stream_sender = self.stream_sender.lock().await;
        stream_sender.send(&message_bytes).await
    }

    // GoAway message is implemented then we can use this function to send GoAway message.
    #[allow(dead_code)]
    pub(crate) async fn close(&self) -> anyhow::Result<()> {
        let mut stream_sender = self.stream_sender.lock().await;
        stream_sender.close().await
    }
}
