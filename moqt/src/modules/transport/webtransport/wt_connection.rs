use async_trait::async_trait;

use crate::modules::transport::transport_connection::TransportConnection;
use super::wt_receive_stream::WtReceiveStream;
use super::wt_send_stream::WtSendStream;

#[derive(Debug)]
pub struct WtConnection;

#[async_trait]
impl TransportConnection for WtConnection {
    type SendStream = WtSendStream;
    type ReceiveStream = WtReceiveStream;

    fn id(&self) -> usize {
        todo!("WebTransport connection id not yet implemented")
    }

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        todo!("WebTransport open_bi not yet implemented")
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        todo!("WebTransport accept_bi not yet implemented")
    }

    async fn open_uni(&self) -> anyhow::Result<Self::SendStream> {
        todo!("WebTransport open_uni not yet implemented")
    }

    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream> {
        todo!("WebTransport accept_uni not yet implemented")
    }

    fn send_datagram(&self, _bytes: bytes::BytesMut) -> anyhow::Result<()> {
        todo!("WebTransport send_datagram not yet implemented")
    }

    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut> {
        todo!("WebTransport receive_datagram not yet implemented")
    }
}
