use bytes::{BufMut, BytesMut};

use crate::{
    TransportProtocol,
    modules::{
        extensions::buf_put_ext::BufPutExt,
        moqt::control_plane::control_messages::control_message_type::ControlMessageType,
        transport::transport_send_stream::TransportSendStream,
    },
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
        let mut message_bytes = BytesMut::new();
        message_bytes.put_varint(message_type as u64);
        message_bytes.put_u16(bytes.len() as u16);
        message_bytes.extend_from_slice(&bytes);
        let mut stream_sender = self.stream_sender.lock().await;
        stream_sender.send(&message_bytes).await
    }

    pub(crate) async fn close(&self) -> anyhow::Result<()> {
        let mut stream_sender = self.stream_sender.lock().await;
        stream_sender.close()
    }
}
