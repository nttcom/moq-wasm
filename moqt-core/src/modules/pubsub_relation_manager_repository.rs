use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};
use crate::models::subscriptions::Subscription;

use anyhow::Result;
use async_trait::async_trait;

use super::models::tracks::ForwardingPreference;

#[async_trait]
pub trait PubSubRelationManagerRepository: Send + Sync {
    async fn setup_publisher(
        &self,
        max_subscribe_id: u64,
        upstream_session_id: usize,
    ) -> Result<()>;
    async fn set_upstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        upstream_session_id: usize,
    ) -> Result<()>;
    async fn set_downstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        downstream_session_id: usize,
    ) -> Result<()>;
    async fn set_downstream_subscribed_namespace_prefix(
        &self,
        track_namespace_prefix: Vec<String>,
        downstream_session_id: usize,
    ) -> Result<()>;
    async fn setup_subscriber(
        &self,
        max_subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<()>;
    async fn is_valid_downstream_subscribe_id(
        &self,
        subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<bool>;
    async fn is_valid_downstream_track_alias(
        &self,
        track_alias: u64,
        downstream_session_id: usize,
    ) -> Result<bool>;
    async fn is_track_existing(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<bool>;
    async fn get_upstream_subscription_by_full_track_name(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<Option<Subscription>>;
    async fn get_downstream_subscription_by_ids(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<Option<Subscription>>;
    async fn get_upstream_session_id(&self, track_namespace: Vec<String>) -> Result<Option<usize>>;
    async fn get_requesting_downstream_session_ids_and_subscribe_ids(
        &self,
        upstream_subscribe_id: u64,
        upstream_session_id: usize,
    ) -> Result<Option<Vec<(usize, u64)>>>;
    async fn get_upstream_subscribe_id(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
        upstream_session_id: usize,
    ) -> Result<Option<u64>>;
    #[allow(clippy::too_many_arguments)]
    async fn set_downstream_subscription(
        &self,
        downstream_session_id: usize,
        subscribe_id: u64,
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
    ) -> Result<()>;
    #[allow(clippy::too_many_arguments)]
    async fn set_upstream_subscription(
        &self,
        upstream_session_id: usize,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
    ) -> Result<(u64, u64)>;
    async fn set_pubsub_relation(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()>;
    async fn activate_downstream_subscription(
        &self,
        downstream_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool>;
    async fn activate_upstream_subscription(
        &self,
        upstream_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool>;
    async fn get_upstream_namespaces_matches_prefix(
        &self,
        track_namespace_prefix: Vec<String>,
    ) -> Result<Vec<Vec<String>>>;
    async fn is_namespace_already_announced(
        &self,
        track_namespace: Vec<String>,
        downstream_session_id: usize,
    ) -> Result<bool>;
    async fn get_downstream_session_ids_by_upstream_namespace(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<Vec<usize>>;
    async fn delete_upstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        upstream_session_id: usize,
    ) -> Result<bool>;
    async fn delete_client(&self, session_id: usize) -> Result<bool>;
    async fn delete_pubsub_relation(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()>;
    async fn delete_upstream_subscription(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<()>;
    async fn delete_downstream_subscription(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()>;
    async fn set_downstream_forwarding_preference(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        forwarding_preference: ForwardingPreference,
    ) -> Result<()>;
    async fn set_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        forwarding_preference: ForwardingPreference,
    ) -> Result<()>;
    async fn get_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Option<ForwardingPreference>>;
    async fn get_related_subscribers(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Vec<(usize, u64)>>;
    async fn get_related_publisher(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<(usize, u64)>;
}
