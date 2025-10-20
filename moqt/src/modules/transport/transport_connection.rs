use crate::modules::transport::{
    transport_receive_stream::TransportReceiveStream, transport_send_stream::TransportSendStream,
};
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransportConnection: Send + Sync {
    type SendStream: TransportSendStream;
    type ReceiveStream: TransportReceiveStream;

    fn id(&self) -> usize;
    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)>;
    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)>;
    async fn open_uni(&self) -> anyhow::Result<Self::SendStream>;
    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream>;
    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()>;
    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut>;
}
