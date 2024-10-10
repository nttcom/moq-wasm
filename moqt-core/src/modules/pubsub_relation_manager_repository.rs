use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};
use crate::models::subscriptions::Subscription;

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
    async fn delete_upstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        upstream_session_id: usize,
    ) -> Result<bool>;
    async fn delete_client(&self, session_id: usize) -> Result<bool>;
}
