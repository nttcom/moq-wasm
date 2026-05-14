use anyhow::bail;
use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::quic::quic_receive_stream::QUICReceiveStream;
use crate::modules::transport::quic::quic_send_stream::QUICSendStream;
use crate::modules::transport::transport_connection::TransportConnection;

#[derive(Debug)]
pub struct QUICConnection {
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

    async fn closed(&self) {
        let reason = self.connection.closed().await;
        tracing::info!("QUIC connection closed: {:?}", reason);
    }

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (sender, receiver) = self.connection.open_bi().await?;
        let send_stream = QUICSendStream {
            send_stream: sender,
        };
        let receive_stream = QUICReceiveStream {
            recv_stream: receiver,
        };
        Ok((send_stream, receive_stream))
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (sender, receiver) = self.connection.accept_bi().await?;
        let send_stream = QUICSendStream {
            send_stream: sender,
        };
        let receive_stream = QUICReceiveStream {
            recv_stream: receiver,
        };
        Ok((send_stream, receive_stream))
    }

    async fn open_uni(&self) -> anyhow::Result<Self::SendStream> {
        let send_stream = self.connection.open_uni().await?;
        Ok(QUICSendStream { send_stream })
    }

    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream> {
        let recv_stream = self.connection.accept_uni().await?;
        Ok(QUICReceiveStream { recv_stream })
    }

    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()> {
        Ok(self.connection.send_datagram(bytes.into())?)
    }

    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut> {
        match self.connection.read_datagram().await {
            Ok(bytes) => Ok(BytesMut::from(&bytes[..])),
            Err(e) => {
                tracing::error!("Failed to receive datagram: {:?}", e);
                bail!("Failed to receive datagram: {:?}", e)
            }
        }
    }
}
