use async_trait::async_trait;

use super::dual_receive_stream::DualReceiveStream;
use super::dual_send_stream::DualSendStream;
use crate::modules::transport::{
    quic::quic_connection::QUICConnection, transport_connection::TransportConnection,
    webtransport::wt_connection::WtConnection,
};

#[derive(Debug)]
pub enum DualConnection {
    Quic(QUICConnection),
    WebTransport(Box<WtConnection>),
}

#[async_trait]
impl TransportConnection for DualConnection {
    type SendStream = DualSendStream;
    type ReceiveStream = DualReceiveStream;

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        match self {
            DualConnection::Quic(c) => {
                let (send, recv) = c.open_bi().await?;
                Ok((DualSendStream::Quic(send), DualReceiveStream::Quic(recv)))
            }
            DualConnection::WebTransport(c) => {
                let (send, recv) = c.open_bi().await?;
                Ok((
                    DualSendStream::WebTransport(send),
                    DualReceiveStream::WebTransport(recv),
                ))
            }
        }
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        match self {
            DualConnection::Quic(c) => {
                let (send, recv) = c.accept_bi().await?;
                Ok((DualSendStream::Quic(send), DualReceiveStream::Quic(recv)))
            }
            DualConnection::WebTransport(c) => {
                let (send, recv) = c.accept_bi().await?;
                Ok((
                    DualSendStream::WebTransport(send),
                    DualReceiveStream::WebTransport(recv),
                ))
            }
        }
    }

    async fn open_uni(&self) -> anyhow::Result<Self::SendStream> {
        match self {
            DualConnection::Quic(c) => Ok(DualSendStream::Quic(c.open_uni().await?)),
            DualConnection::WebTransport(c) => {
                Ok(DualSendStream::WebTransport(c.open_uni().await?))
            }
        }
    }

    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream> {
        match self {
            DualConnection::Quic(c) => Ok(DualReceiveStream::Quic(c.accept_uni().await?)),
            DualConnection::WebTransport(c) => {
                Ok(DualReceiveStream::WebTransport(c.accept_uni().await?))
            }
        }
    }

    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()> {
        match self {
            DualConnection::Quic(c) => c.send_datagram(bytes),
            DualConnection::WebTransport(c) => c.send_datagram(bytes),
        }
    }

    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut> {
        match self {
            DualConnection::Quic(c) => c.receive_datagram().await,
            DualConnection::WebTransport(c) => c.receive_datagram().await,
        }
    }

    async fn closed(&self) {
        match self {
            DualConnection::Quic(c) => c.closed().await,
            DualConnection::WebTransport(c) => c.closed().await,
        }
    }
}
