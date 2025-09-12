use bytes::BytesMut;

use crate::modules::{moqt::protocol::TransportProtocol, transport::transport_send_stream::TransportSendStream};

pub(crate) struct ControlSender<T: TransportProtocol> {
    pub(crate) send_stream: T::SendStream
}

impl<T: TransportProtocol> ControlSender<T> {
    pub(crate) async fn send(&mut self, bytes: &BytesMut) -> anyhow::Result<()> {
        self.send_stream.send(bytes).await
    }
}