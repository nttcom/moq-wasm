use std::{fmt::Debug, sync::Arc};

use dashmap::DashSet;
use uuid::Uuid;

use crate::modules::core::handler::publish::PublishHandler;

#[async_trait::async_trait]
pub(crate) trait Table: Send + Sync + 'static + Debug {
    fn new() -> Self
    where
        Self: Sized;
    fn register_publish_namespace(&self, session_id: Uuid, track_namespace: String) -> bool;
    fn register_subscribe_namespace(&self, session_id: Uuid, track_namespace_prefix: String);
    async fn register_publish(&self, session_id: Uuid, handler: Arc<dyn PublishHandler>);
    fn get_namespace_subscribers(&self, track_namespace: &str) -> DashSet<Uuid>;
    async fn get_subscribers(
        &self,
        track_namespace_prefix: &str,
    ) -> DashSet<(String, (Option<String>, Option<u64>))>;
    fn get_publish_namespace(&self, track_namespace: &str) -> Option<Uuid>;
    async fn find_publish_handler_with(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<(Uuid, Arc<dyn PublishHandler>)>;
}
