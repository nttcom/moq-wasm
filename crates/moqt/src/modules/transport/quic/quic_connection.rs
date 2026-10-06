use crate::modules::transport::transport_connection::{BoxedReceiveStream, BoxedSendStream};
use anyhow::bail;
use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::quic::quic_receive_stream::QUICReceiveStream;
use crate::modules::transport::quic::quic_send_stream::QUICSendStream;
use crate::modules::transport::transport_connection::{TransportClose, TransportConnection};
use crate::modules::transport::transport_stats::TransportStats;

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
    async fn closed(&self) -> TransportClose {
        let error = self.connection.closed().await;
        tracing::info!("QUIC connection closed: {:?}", error);
        match error {
            quinn::ConnectionError::ApplicationClosed(close) => TransportClose {
                code: u32::try_from(close.error_code.into_inner()).ok(),
                reason: String::from_utf8_lossy(&close.reason).into_owned(),
            },
            other => TransportClose {
                code: None,
                reason: other.to_string(),
            },
        }
    }

    fn close(&self, code: u32, reason: &str) {
        self.connection
            .close(quinn::VarInt::from_u32(code), reason.as_bytes());
        tracing::info!(code, reason, "QUIC connection close requested");
    }

    async fn open_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)> {
        let (sender, receiver) = self.connection.open_bi().await?;
        Ok((
            Box::new(QUICSendStream {
                send_stream: sender,
            }),
            Box::new(QUICReceiveStream {
                recv_stream: receiver,
            }),
        ))
    }

    async fn accept_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)> {
        let (sender, receiver) = self.connection.accept_bi().await?;
        Ok((
            Box::new(QUICSendStream {
                send_stream: sender,
            }),
            Box::new(QUICReceiveStream {
                recv_stream: receiver,
            }),
        ))
    }

    async fn open_uni(&self) -> anyhow::Result<BoxedSendStream> {
        let send_stream = self.connection.open_uni().await?;
        Ok(Box::new(QUICSendStream { send_stream }))
    }

    async fn accept_uni(&self) -> anyhow::Result<BoxedReceiveStream> {
        let recv_stream = self.connection.accept_uni().await?;
        Ok(Box::new(QUICReceiveStream { recv_stream }))
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

    fn stats(&self) -> TransportStats {
        self.connection.stats().into()
    }
}
