use async_trait::async_trait;

use crate::modules::transport::quic::quic_receive_stream::QUICReceiveStream;
use crate::modules::transport::quic::quic_send_stream::QUICSendStream;
use crate::modules::transport::transport_connection::TransportConnection;

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
    type SendStream = QUICSendStream;
    type ReceiveStream = QUICReceiveStream;

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (sender, receiver) = self.connection.open_bi().await?;
        let send_stream =
            QUICSendStream::new(self.connection.stable_id(), receiver.id().into(), sender);
        let receive_stream =
            QUICReceiveStream::new(self.connection.stable_id(), receiver.id().into(), receiver);
        Ok((send_stream, receive_stream))
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (sender, receiver) = self.connection.accept_bi().await?;
        let send_stream =
            QUICSendStream::new(self.connection.stable_id(), receiver.id().into(), sender);
        let receive_stream =
            QUICReceiveStream::new(self.connection.stable_id(), receiver.id().into(), receiver);
        Ok((send_stream, receive_stream))
    }
}
