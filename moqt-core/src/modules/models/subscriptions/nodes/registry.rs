use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};
use crate::models::subscriptions::Subscription;
use crate::models::tracks::ForwardingPreference;
use anyhow::Result;

type SubscribeId = u64;
type TrackNamespace = Vec<String>;
type TrackAlias = u64;

pub trait SubscriptionNodeRegistry {
    #[allow(clippy::too_many_arguments)]
    fn set_subscription(
        &mut self,
        subscribe_id: SubscribeId,
        track_alias: u64,
        track_namespace: TrackNamespace,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
    ) -> Result<()>;
    fn get_subscription(&self, subscribe_id: SubscribeId) -> Result<Option<Subscription>>;
    fn get_subscription_by_full_track_name(
        &self,
        track_namespace: TrackNamespace,
        track_name: String,
    ) -> Result<Option<Subscription>>;
    fn get_subscribe_id(
        &self,
        track_namespace: TrackNamespace,
        track_name: String,
    ) -> Result<Option<SubscribeId>>;
    fn has_track(&self, track_namespace: TrackNamespace, track_name: String) -> bool;
    fn activate_subscription(&mut self, subscribe_id: SubscribeId) -> Result<bool>;
    fn is_requesting(&self, subscribe_id: SubscribeId) -> bool;
    fn delete_subscription(&mut self, subscribe_id: SubscribeId) -> Result<()>;
    fn set_forwarding_preference(
        &mut self,
        subscribe_id: SubscribeId,
        forwarding_preference: ForwardingPreference,
    ) -> Result<()>;
    fn get_forwarding_preference(
        &self,
        subscribe_id: SubscribeId,
    ) -> Result<Option<ForwardingPreference>>;
    fn get_filter_type(&self, subscribe_id: SubscribeId) -> Result<FilterType>;
    fn get_absolute_start(&self, subscribe_id: SubscribeId) -> Result<(Option<u64>, Option<u64>)>;
    fn get_absolute_end(&self, subscribe_id: SubscribeId) -> Result<(Option<u64>, Option<u64>)>;

    fn is_subscribe_id_valid(&self, subscribe_id: SubscribeId) -> bool;
    fn is_track_alias_valid(&self, track_alias: TrackAlias) -> bool;
    fn create_valid_track_alias(&self) -> Result<TrackAlias>;
    fn create_latest_subscribe_id_and_track_alias(&self) -> Result<(SubscribeId, TrackAlias)>;

    fn set_namespace(&mut self, namespace: TrackNamespace) -> Result<()>;
    fn get_namespaces(&self) -> Result<&Vec<TrackNamespace>>;
    fn has_namespace(&self, namespace: TrackNamespace) -> bool;
    fn delete_namespace(&mut self, namespace: TrackNamespace) -> Result<()>;

    fn set_namespace_prefix(&mut self, namespace_prefix: TrackNamespace) -> Result<()>;
    fn get_namespace_prefixes(&self) -> Result<&Vec<TrackNamespace>>;
    fn delete_namespace_prefix(&mut self, namespace_prefix: TrackNamespace) -> Result<()>;
}
