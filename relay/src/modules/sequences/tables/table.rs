use std::{fmt::Debug, sync::Arc};

use dashmap::DashSet;

use crate::modules::{core::handler::publish::PublishHandler, types::SessionId};

#[async_trait::async_trait]
pub(crate) trait Table: Send + Sync + 'static + Debug {
    fn new() -> Self
    where
        Self: Sized;
    fn register_publish_namespace(&self, session_id: SessionId, track_namespace: String) -> bool;
    fn register_subscribe_namespace(&self, session_id: SessionId, track_namespace_prefix: String);
    async fn register_publish(&self, session_id: SessionId, handler: Arc<dyn PublishHandler>);
    fn get_namespace_subscribers(&self, track_namespace: &str) -> DashSet<SessionId>;
    async fn get_subscribers(
        &self,
        track_namespace_prefix: &str,
    ) -> DashSet<(String, (Option<String>, Option<u64>))>;
    fn get_publish_namespace(&self, track_namespace: &str) -> Option<SessionId>;
    async fn find_publish_handler_with(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<(SessionId, Arc<dyn PublishHandler>)>;
}
