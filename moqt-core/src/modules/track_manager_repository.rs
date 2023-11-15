use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TrackManagerRepository: Send + Sync {
    async fn set(&self, track_namespace: &str) -> Result<()>;
    async fn delete(&self, track_namespace: &str) -> Result<()>;
    async fn has(&self, track_namespace: &str) -> bool;
}
