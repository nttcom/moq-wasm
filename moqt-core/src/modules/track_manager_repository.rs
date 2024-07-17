use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TrackManagerRepository: Send + Sync {
    async fn set(&self, track_namespace: &str, session_id: usize) -> Result<()>;
    async fn delete(&self, track_namespace: &str) -> Result<()>;
    async fn has(&self, track_namespace: &str) -> bool;
    async fn get_publisher_session_id_by_track_namespace(
        &self,
        track_namespace: &str,
    ) -> Option<usize>;
}
