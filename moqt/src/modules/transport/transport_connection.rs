use crate::modules::transport::{
    transport_receive_stream::TransportReceiveStream, transport_send_stream::TransportSendStream,
};
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransportConnection: Send + Sync {
    async fn open_bi(
        &self,
    ) -> anyhow::Result<(
        Box<dyn TransportSendStream>,
        Box<tokio::sync::Mutex<dyn TransportReceiveStream>>,
    )>;
    async fn accept_bi(
        &self,
    ) -> anyhow::Result<(
        Box<dyn TransportSendStream>,
        Box<tokio::sync::Mutex<dyn TransportReceiveStream>>,
    )>;
}
