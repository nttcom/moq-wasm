use anyhow::Result;
use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
use moqt_core::models::subscriptions::Subscription;
use tokio::sync::oneshot;

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
    SetupSubscriber {
        max_subscribe_id: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    IsValidDownstreamSubscribeId {
        subscribe_id: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsValidDownstreamTrackAlias {
        track_alias: u64,
        downstream_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsTrackExisting {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<Result<bool>>,
    },
    GetUpstreamSubscription {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<Result<Option<Subscription>>>,
    },
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
        end_object: Option<u64>,
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
        end_object: Option<u64>,
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
    #[cfg(test)]
    GetNodeAndRelationClone {
        resp: oneshot::Sender<Result<(Consumers, Producers, PubSubRelation)>>,
    },
}
