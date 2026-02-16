use async_trait::async_trait;

use crate::modules::transport::transport_connection::TransportConnection;
use super::wt_receive_stream::WtReceiveStream;
use super::wt_send_stream::WtSendStream;

#[derive(Debug)]
pub struct WtConnection {
    connection: wtransport::Connection,
}

impl WtConnection {
    pub(crate) fn new(connection: wtransport::Connection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl TransportConnection for WtConnection {
    type SendStream = WtSendStream;
    type ReceiveStream = WtReceiveStream;

    fn id(&self) -> usize {
        self.connection.stable_id()
    }

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (send, recv) = self.connection.open_bi().await?.await?;
        let send_stream = WtSendStream {
            stable_id: self.connection.stable_id(),
            stream_id: recv.id().into_u64(),
            send_stream: send,
        };
        let receive_stream = WtReceiveStream {
            stable_id: self.connection.stable_id(),
            stream_id: recv.id().into_u64(),
            recv_stream: recv,
        };
        Ok((send_stream, receive_stream))
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (send, recv) = self.connection.accept_bi().await?;
        let send_stream = WtSendStream {
            stable_id: self.connection.stable_id(),
            stream_id: recv.id().into_u64(),
            send_stream: send,
        };
        let receive_stream = WtReceiveStream {
            stable_id: self.connection.stable_id(),
            stream_id: recv.id().into_u64(),
            recv_stream: recv,
        };
        Ok((send_stream, receive_stream))
    }

    async fn open_uni(&self) -> anyhow::Result<Self::SendStream> {
        let send = self.connection.open_uni().await?.await?;
        Ok(WtSendStream {
            stable_id: self.connection.stable_id(),
            stream_id: send.id().into_u64(),
            send_stream: send,
        })
    }

    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream> {
        let recv = self.connection.accept_uni().await?;
        Ok(WtReceiveStream {
            stable_id: self.connection.stable_id(),
            stream_id: recv.id().into_u64(),
            recv_stream: recv,
        })
    }

    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()> {
        Ok(self.connection.send_datagram(bytes)?)
    }

    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut> {
        match self.connection.receive_datagram().await {
            Ok(datagram) => Ok(bytes::BytesMut::from(&datagram[..])),
            Err(e) => {
                tracing::error!("Failed to receive datagram: {:?}", e);
                anyhow::bail!("Failed to receive datagram: {:?}", e)
            }
        }
    }
}
