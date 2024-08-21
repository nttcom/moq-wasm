use crate::message_handler::StreamType;
use crate::messages::moqt_payload::MOQTPayload;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait RelayHandlerManagerRepository: Send + Sync {
    async fn broadcast_message_to_relay_handlers(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()>;
    async fn send_message_to_relay_handler(
        &self,
        session_id: usize,
        message: Box<dyn MOQTPayload>,
        stream_type: StreamType,
    ) -> Result<()>;
}
