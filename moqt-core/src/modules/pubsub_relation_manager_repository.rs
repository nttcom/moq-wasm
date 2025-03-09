use super::models::range::{ObjectRange, ObjectStart};
use crate::{
    messages::control_messages::subscribe::{FilterType, GroupOrder},
    models::tracks::ForwardingPreference,
};
use anyhow::Result;
use async_trait::async_trait;

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
    async fn is_downstream_subscribe_id_unique(
        &self,
        subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<bool>;
    async fn is_downstream_subscribe_id_less_than_max(
        &self,
        subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<bool>;
    async fn is_downstream_track_alias_unique(
        &self,
        track_alias: u64,
        downstream_session_id: usize,
    ) -> Result<bool>;
    async fn is_track_existing(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<bool>;
    async fn get_upstream_session_id(&self, track_namespace: Vec<String>) -> Result<Option<usize>>;
    async fn get_requesting_downstream_session_ids_and_subscribe_ids(
        &self,
        upstream_subscribe_id: u64,
        upstream_session_id: usize,
    ) -> Result<Option<Vec<(usize, u64)>>>;
    // TODO: Unify getter methods of subscribe_id
    async fn get_upstream_subscribe_id(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
        upstream_session_id: usize,
    ) -> Result<Option<u64>>;
    async fn get_downstream_track_alias(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<Option<u64>>;
    async fn get_upstream_subscribe_id_by_track_alias(
        &self,
        upstream_session_id: usize,
        upstream_track_alias: u64,
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
    async fn is_namespace_announced(
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
    async fn get_upstream_filter_type(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Option<FilterType>>;
    async fn get_downstream_filter_type(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<Option<FilterType>>;
    async fn get_upstream_requested_object_range(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Option<ObjectRange>>;
    async fn get_downstream_requested_object_range(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<Option<ObjectRange>>;
    async fn set_downstream_actual_object_start(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        actual_object_start: ObjectStart,
    ) -> Result<()>;
    async fn get_downstream_actual_object_start(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<Option<ObjectStart>>;
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
