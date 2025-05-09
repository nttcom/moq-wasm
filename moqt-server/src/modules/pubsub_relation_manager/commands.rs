use anyhow::Result;
use tokio::sync::oneshot;

use moqt_core::{
    messages::control_messages::{group_order::GroupOrder, subscribe::FilterType},
    models::{
        range::{ObjectRange, ObjectStart},
        tracks::ForwardingPreference,
    },
};

#[cfg(test)]
use crate::modules::pubsub_relation_manager::{
    manager::{Consumers, Producers},
    relation::PubSubRelation,
};

#[derive(Debug)]
pub(crate) enum PubSubRelationCommand {
    SetupPublisher {
        max_subscribe_id: u64,
        upstream_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    SetUpstreamAnnouncedNamespace {
        track_namespace: Vec<String>,
        upstream_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    SetDownstreamAnnouncedNamespace {
        track_namespace: Vec<String>,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    SetDownstreamSubscribedNamespacePrefix {
        track_namespace_prefix: Vec<String>,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    SetupSubscriber {
        max_subscribe_id: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    IsDownstreamSubscribeIdUnique {
        subscribe_id: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsDownstreamSubscribeIdLessThanMax {
        subscribe_id: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsDownstreamTrackAliasUnique {
        track_alias: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsUpstreamSubscribed {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<Result<bool>>,
    },
    // TODO: Unify getter methods of subscribe_id
    GetUpstreamSessionId {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<Result<Option<usize>>>,
    },
    GetRequestingDownstreamSessionIdsAndSubscribeIds {
        upstream_subscribe_id: u64,
        upstream_session_id: usize,
        #[allow(clippy::type_complexity)]
        resp: oneshot::Sender<Result<Option<Vec<(usize, u64)>>>>,
    },
    GetUpstreamSubscribeId {
        track_namespace: Vec<String>,
        track_name: String,
        upstream_session_id: usize,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    GetDownstreamTrackAlias {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    GetUpstreamSubscribeIdByTrackAlias {
        upstream_session_id: usize,
        upstream_track_alias: u64,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    SetDownstreamSubscription {
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
        resp: oneshot::Sender<Result<()>>,
    },
    SetUpstreamSubscription {
        upstream_session_id: usize,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        resp: oneshot::Sender<Result<(u64, u64)>>,
    },
    SetPubSubRelation {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    ActivateDownstreamSubscription {
        downstream_session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<bool>>,
    },
    ActivateUpstreamSubscription {
        upstream_session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<bool>>,
    },
    GetUpstreamNamespacesMatchesPrefix {
        track_namespace_prefix: Vec<String>,
        resp: oneshot::Sender<Result<Vec<Vec<String>>>>,
    },
    IsNamespaceAlreadyAnnounced {
        track_namespace: Vec<String>,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    GetDownstreamSessionIdsByUpstreamNamespace {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<Result<Vec<usize>>>,
    },
    DeleteUpstreamAnnouncedNamespace {
        track_namespace: Vec<String>,
        upstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    DeleteClient {
        session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    DeletePubSubRelation {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    DeleteUpstreamSubscription {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    DeleteDownstreamSubscription {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    SetDownstreamForwardingPreference {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        forwarding_preference: ForwardingPreference,
        resp: oneshot::Sender<Result<()>>,
    },
    SetUpstreamForwardingPreference {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        forwarding_preference: ForwardingPreference,
        resp: oneshot::Sender<Result<()>>,
    },
    GetUpstreamForwardingPreference {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<ForwardingPreference>>>,
    },
    GetUpstreamFilterType {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<FilterType>>>,
    },
    GetDownstreamFilterType {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<FilterType>>>,
    },
    GetUpstreamRequestedObjectRange {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<ObjectRange>>>,
    },
    GetDownstreamRequestedObjectRange {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<ObjectRange>>>,
    },
    SetDownstreamActualObjectStart {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        actual_object_start: ObjectStart,
        resp: oneshot::Sender<Result<()>>,
    },
    GetDownstreamActualObjectStart {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<ObjectStart>>>,
    },
    SetUpstreamStreamId {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        group_id: u64,
        subgroup_id: u64,
        stream_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    GetUpstreamSubscribeIdsForClient {
        upstream_session_id: usize,
        resp: oneshot::Sender<Result<Vec<u64>>>,
    },
    GetUpstreamGroupIdsForSubscription {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Vec<u64>>>,
    },
    GetUpstreamSubgroupIdsForGroup {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        group_id: u64,
        resp: oneshot::Sender<Result<Vec<u64>>>,
    },
    GetUpstreamStreamIdForSubgroup {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    SetDownstreamStreamId {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        group_id: u64,
        subgroup_id: u64,
        stream_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    GetDownstreamSubscribeIdsForClient {
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<Vec<u64>>>,
    },
    GetDownstreamGroupIdsForSubscription {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Vec<u64>>>,
    },
    GetDownstreamSubgroupIdsForGroup {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        group_id: u64,
        resp: oneshot::Sender<Result<Vec<u64>>>,
    },
    GetDownstreamStreamIdForSubgroup {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    GetRelatedSubscribers {
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<Vec<(usize, u64)>>>,
    },
    GetRelatedPublisher {
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        resp: oneshot::Sender<Result<(usize, u64)>>,
    },
    #[cfg(test)]
    GetNodeAndRelationClone {
        resp: oneshot::Sender<Result<(Consumers, Producers, PubSubRelation)>>,
    },
}
