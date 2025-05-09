use crate::{
    messages::control_messages::{group_order::GroupOrder, subscribe::FilterType},
    models::{
        range::{ObjectRange, ObjectStart},
        subscriptions::Subscription,
        tracks::ForwardingPreference,
    },
};
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
    ) -> Result<()>;
    fn get_subscription(&self, subscribe_id: SubscribeId) -> Result<Option<Subscription>>;
    // TODO: Unify getter methods of subscribe_id
    fn get_subscribe_id(
        &self,
        track_namespace: TrackNamespace,
        track_name: String,
    ) -> Result<Option<SubscribeId>>;
    fn get_track_alias(&self, subscribe_id: SubscribeId) -> Result<Option<TrackAlias>>;
    fn get_subscribe_id_by_track_alias(
        &self,
        track_alias: TrackAlias,
    ) -> Result<Option<SubscribeId>>;
    fn get_all_subscribe_ids(&self) -> Result<Vec<SubscribeId>>;
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
    fn get_filter_type(&self, subscribe_id: SubscribeId) -> Result<Option<FilterType>>;
    fn set_stream_id(
        &mut self,
        subscribe_id: SubscribeId,
        group_id: u64,
        subgroup_id: u64,
        stream_id: u64,
    ) -> Result<()>;
    fn get_group_ids_for_subscription(&self, subscribe_id: SubscribeId) -> Result<Vec<u64>>;
    fn get_subgroup_ids_for_group(
        &self,
        subscribe_id: SubscribeId,
        group_id: u64,
    ) -> Result<Vec<u64>>;
    fn get_stream_id_for_subgroup(
        &self,
        subscribe_id: SubscribeId,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<Option<u64>>;
    fn get_requested_object_range(&self, subscribe_id: SubscribeId) -> Result<Option<ObjectRange>>;
    fn set_actual_object_start(
        &mut self,
        subscribe_id: SubscribeId,
        actual_object_start: ObjectStart,
    ) -> Result<()>;
    fn get_actual_object_start(&self, subscribe_id: SubscribeId) -> Result<Option<ObjectStart>>;

    fn is_subscribe_id_unique(&self, subscribe_id: SubscribeId) -> bool;
    fn is_subscribe_id_less_than_max(&self, subscribe_id: SubscribeId) -> bool;
    fn is_track_alias_unique(&self, track_alias: TrackAlias) -> bool;
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
