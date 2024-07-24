use anyhow::Result;
use async_trait::async_trait;

use crate::messages::moqt_payload::MOQTPayload;

#[async_trait]
pub trait StreamManagerRepository: Send + Sync {
    // TODO: SUBSCRIBE・OBJECTメッセージの際に関数名を修正する
    async fn broadcast_message(
        &self,
        session_id: Option<usize>,
        message: Box<dyn MOQTPayload>,
    ) -> Result<()>;
    async fn relay_message(&self, session_id: usize, message: Box<dyn MOQTPayload>) -> Result<()>;
}
