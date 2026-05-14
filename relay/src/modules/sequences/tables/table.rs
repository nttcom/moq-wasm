use std::{fmt::Debug, sync::Arc};

use dashmap::DashSet;

use crate::modules::{
    core::handler::publish::PublishHandler,
    enums::ContentExists,
    types::{SessionId, TrackKey},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct UpstreamSubscriptionKey {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveUpstreamSubscription {
    pub(crate) upstream_subscribe_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) expires: u64,
    pub(crate) content_exists: ContentExists,
    pub(crate) downstream_subscriber_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct RemovedDownstreamSubscription {
    pub(crate) upstream_key: UpstreamSubscriptionKey,
    pub(crate) upstream_subscribe_id: u64,
    pub(crate) remaining_downstream_subscriber_count: usize,
}

#[async_trait::async_trait]
pub(crate) trait SignalingStateTable: Send + Sync + 'static + Debug {
    fn new() -> Self
    where
        Self: Sized;
    async fn remove_session(&self, session_id: SessionId);
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
    fn register_track_alias_link(
        &self,
        publisher_session_id: SessionId,
        publisher_track_alias: u64,
        subscriber_session_id: SessionId,
        subscriber_track_alias: u64,
    );
    fn _find_subscriber_track_alias(
        &self,
        publisher_session_id: SessionId,
        publisher_track_alias: u64,
        subscriber_session_id: SessionId,
    ) -> Option<u64>;
    fn get_active_upstream_subscription(
        &self,
        publisher_session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<ActiveUpstreamSubscription>;
    fn register_upstream_subscription(
        &self,
        key: UpstreamSubscriptionKey,
        subscription: ActiveUpstreamSubscription,
    );
    fn register_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        upstream_key: UpstreamSubscriptionKey,
    ) -> bool;
    fn remove_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<RemovedDownstreamSubscription>;
}
