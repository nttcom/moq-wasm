use crate::modules::transport::{
    transport_receive_stream::TransportReceiveStream, transport_send_stream::TransportSendStream,
};
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransportConnection: Send + Sync {
    type SendStream: TransportSendStream;
    type ReceiveStream: TransportReceiveStream;

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)>;
    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)>;
}
