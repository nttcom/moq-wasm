use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TrackNamespaceManagerRepository: Send + Sync {
    async fn set_publisher(&self, track_namespace: &str, session_id: usize) -> Result<()>;
    async fn delete_publisher_by_namespace(&self, track_namespace: &str) -> Result<()>;
    async fn delete_publisher_by_session_id(&self, publisher_session_id: usize) -> Result<()>;
    async fn has_track_namespace(&self, track_namespace: &str) -> bool;
    async fn has_track_name(&self, track_namespace: &str, track_name: &str) -> bool;
    async fn get_publisher_session_id_by_track_namespace(
        &self,
        track_namespace: &str,
    ) -> Option<usize>;
    async fn set_subscriber(
        &self,
        track_namespace: &str,
        subscriber_session_id: usize,
        track_name: &str,
    ) -> Result<()>;
    async fn delete_subscriber(
        &self,
        track_namespace: &str,
        track_name: &str,
        subscriber_session_id: usize,
    ) -> Result<()>;
    async fn delete_subscribers_by_session_id(&self, subscriber_session_id: usize) -> Result<()>;
    async fn set_track_id(
        &self,
        track_namespace: &str,
        track_name: &str,
        track_id: u64,
    ) -> Result<()>;
    async fn activate_subscriber(
        &self,
        track_namespace: &str,
        track_name: &str,
        subscriber_session_id: usize,
    ) -> Result<()>;
    async fn get_subscriber_session_ids_by_track_namespace_and_track_name(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<Vec<usize>>;
    async fn get_subscriber_session_ids_by_track_id(&self, track_id: u64) -> Option<Vec<usize>>;
    async fn delete_client(&self, session_id: usize) -> Result<()>;
}
