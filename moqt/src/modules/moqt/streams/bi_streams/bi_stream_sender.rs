use bytes::BytesMut;

use crate::modules::{
    moqt::protocol::TransportProtocol, transport::transport_send_stream::TransportSendStream,
};

pub(crate) struct BiStreamSender<T: TransportProtocol> {
    send_stream: tokio::sync::Mutex<T::SendStream>,
}

impl<T: TransportProtocol> BiStreamSender<T> {
    pub(crate) fn new(send_stream: T::SendStream) -> Self {
        Self {
            send_stream: tokio::sync::Mutex::new(send_stream),
        }
    }

    pub(crate) async fn send(&self, bytes: &BytesMut) -> anyhow::Result<()> {
        self.send_stream.lock().await.send(bytes).await
    }
}
