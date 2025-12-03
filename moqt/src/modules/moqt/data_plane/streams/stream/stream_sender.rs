use bytes::BytesMut;

use crate::modules::{
    moqt::protocol::TransportProtocol, transport::transport_send_stream::TransportSendStream,
};

#[derive(Debug)]
pub struct StreamSender<T: TransportProtocol> {
    send_stream: tokio::sync::Mutex<T::SendStream>,
}

impl<T: TransportProtocol> StreamSender<T> {
    pub(crate) fn new(send_stream: T::SendStream) -> Self {
        Self {
            send_stream: tokio::sync::Mutex::new(send_stream),
        }
    }

    pub async fn send(&self, bytes: &BytesMut) -> anyhow::Result<()> {
        self.send_stream.lock().await.send(bytes).await
    }
}
