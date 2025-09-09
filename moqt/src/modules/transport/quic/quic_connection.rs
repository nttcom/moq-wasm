use async_trait::async_trait;

use crate::modules::transport::quic::quic_receive_stream::QUICReceiveStream;
use crate::modules::transport::quic::quic_send_stream::QUICSendStream;
use crate::modules::transport::transport_connection::TransportConnection;
use crate::modules::transport::transport_receive_stream::TransportReceiveStream;
use crate::modules::transport::transport_send_stream::TransportSendStream;

pub(crate) struct QUICConnection {
    connection: quinn::Connection,
}

impl QUICConnection {
    pub(crate) fn new(connection: quinn::Connection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl TransportConnection for QUICConnection {
    async fn open_bi(
        &self,
    ) -> anyhow::Result<(
        Box<dyn TransportSendStream>,
        Box<tokio::sync::Mutex<dyn TransportReceiveStream>>,
    )> {
        let (sender, receiver) = self.connection.open_bi().await?;
        let send_stream =
            QUICSendStream::new(self.connection.stable_id(), receiver.id().into(), sender);
        let receive_stream =
            QUICReceiveStream::new(self.connection.stable_id(), receiver.id().into(), receiver);
        Ok((Box::new(send_stream), Box::new(tokio::sync::Mutex::new(receive_stream))))
    }

    async fn accept_bi(
        &self,
    ) -> anyhow::Result<(
        Box<dyn TransportSendStream>,
        Box<tokio::sync::Mutex<dyn TransportReceiveStream>>,
    )> {
        let (sender, receiver) = self.connection.accept_bi().await?;
        let send_stream =
            QUICSendStream::new(self.connection.stable_id(), receiver.id().into(), sender);
        let receive_stream =
            QUICReceiveStream::new(self.connection.stable_id(), receiver.id().into(), receiver);
        Ok((Box::new(send_stream), Box::new(tokio::sync::Mutex::new(receive_stream))))
    }
}
