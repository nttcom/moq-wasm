use bytes::BytesMut;

use crate::modules::transport::transport_send_stream::TransportSendStream;

pub(crate) struct MOQTControlSender {
    transport_stream: Box<dyn TransportSendStream>
}

impl MOQTControlSender {
    pub(crate) fn new(transport_stream: Box<dyn TransportSendStream>) -> Self {
        Self {
            transport_stream
        }
    }

    pub(crate) async fn send(&mut self, bytes: &BytesMut) -> anyhow::Result<()> {
        self.transport_stream.send(bytes).await
    }
}