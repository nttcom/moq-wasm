use anyhow::Result;
use async_trait::async_trait;

use crate::messages::moqt_payload::MOQTPayload;

#[async_trait]
pub trait StreamManagerRepository: Send + Sync {
    async fn broadcast_message(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()>;
}
