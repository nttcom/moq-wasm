use crate::messages::moqt_payload::MOQTPayload;
use crate::stream_type::StreamType;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait SendStreamDispatcherRepository: Send + Sync {
    async fn broadcast_message_to_send_stream_threads(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()>;
    async fn send_message_to_send_stream_thread(
        &self,
        session_id: usize,
        message: Box<dyn MOQTPayload>,
        stream_type: StreamType,
    ) -> Result<()>;
}
