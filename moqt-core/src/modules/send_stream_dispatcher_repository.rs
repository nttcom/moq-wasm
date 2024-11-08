use anyhow::Result;
use async_trait::async_trait;

use crate::{constants::StreamDirection, messages::moqt_payload::MOQTPayload};

#[async_trait]
pub trait SendStreamDispatcherRepository: Send + Sync {
    async fn broadcast_message_to_send_stream_threads(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()>;
    async fn forward_message_to_send_stream_thread(
        &self,
        session_id: usize,
        message: Box<dyn MOQTPayload>,
        stream_direction: StreamDirection,
    ) -> Result<()>;
}
