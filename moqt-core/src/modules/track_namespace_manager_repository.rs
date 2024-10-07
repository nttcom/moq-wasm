use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};
use crate::subscription_models::subscriptions::Subscription;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TrackNamespaceManagerRepository: Send + Sync {
    async fn setup_publisher(
        &self,
        max_subscribe_id: u64,
        publisher_session_id: usize,
    ) -> Result<()>;
    async fn set_publisher_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<()>;
    async fn setup_subscriber(
        &self,
        max_subscribe_id: u64,
        subscriber_session_id: usize,
    ) -> Result<()>;
    async fn is_valid_subscriber_subscribe_id(
        &self,
        subscribe_id: u64,
        subscriber_session_id: usize,
    ) -> Result<bool>;
    async fn is_valid_subscriber_track_alias(
        &self,
        track_alias: u64,
        subscriber_session_id: usize,
    ) -> Result<bool>;
    async fn is_track_existing(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<bool>;
    async fn get_publisher_subscription_by_full_track_name(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<Option<Subscription>>;
    async fn get_publisher_session_id(&self, track_namespace: Vec<String>)
        -> Result<Option<usize>>;
    async fn get_requesting_subscriber_session_ids_and_subscribe_ids(
        &self,
        publisher_subscribe_id: u64,
        publisher_session_id: usize,
    ) -> Result<Option<Vec<(usize, u64)>>>;
    async fn get_publisher_subscribe_id(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
        publisher_session_id: usize,
    ) -> Result<Option<u64>>;
    #[allow(clippy::too_many_arguments)]
    async fn set_subscriber_subscription(
        &self,
        subscriber_session_id: usize,
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
    async fn set_publisher_subscription(
        &self,
        publisher_session_id: usize,
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
    async fn register_pubsup_relation(
        &self,
        publisher_session_id: usize,
        publisher_subscribe_id: u64,
        subscriber_session_id: usize,
        subscriber_subscribe_id: u64,
    ) -> Result<()>;
    async fn activate_subscriber_subscription(
        &self,
        subscriber_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool>;
    async fn activate_publisher_subscription(
        &self,
        publisher_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool>;
    async fn delete_publisher_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<bool>;
    async fn delete_client(&self, session_id: usize) -> Result<bool>;
}
