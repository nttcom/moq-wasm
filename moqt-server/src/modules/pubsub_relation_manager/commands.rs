use anyhow::Result;
use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
use moqt_core::subscription_models::subscriptions::Subscription;
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
        publisher_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    SetPublisherAnnouncedNamespace {
        track_namespace: Vec<String>,
        publisher_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    SetupSubscriber {
        max_subscribe_id: u64,
        subscriber_session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
    IsValidSubscriberSubscribeId {
        subscribe_id: u64,
        subscriber_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsValidSubscriberTrackAlias {
        track_alias: u64,
        subscriber_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    IsTrackExisting {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<Result<bool>>,
    },
    GetPublisherSubscription {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<Result<Option<Subscription>>>,
    },
    GetPublisherSessionId {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<Result<Option<usize>>>,
    },
    GetRequestingSubscriberSessionIdsAndSubscribeIds {
        publisher_subscribe_id: u64,
        publisher_session_id: usize,
        #[allow(clippy::type_complexity)]
        resp: oneshot::Sender<Result<Option<Vec<(usize, u64)>>>>,
    },
    GetPublisherSubscribeId {
        track_namespace: Vec<String>,
        track_name: String,
        publisher_session_id: usize,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    SetSubscriberSubscription {
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
        resp: oneshot::Sender<Result<()>>,
    },
    SetPublisherSubscription {
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
        resp: oneshot::Sender<Result<(u64, u64)>>,
    },
    RegisterPubSubRelation {
        publisher_session_id: usize,
        publisher_subscribe_id: u64,
        subscriber_session_id: usize,
        subscriber_subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    ActivateSubscriberSubscription {
        subscriber_session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<bool>>,
    },
    ActivatePublisherSubscription {
        publisher_session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<bool>>,
    },
    DeletePublisherAnnouncedNamespace {
        track_namespace: Vec<String>,
        publisher_session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    DeleteClient {
        session_id: usize,
        resp: oneshot::Sender<Result<bool>>,
    },
    #[cfg(test)]
    GetNodeAndRelationClone {
        resp: oneshot::Sender<Result<(Consumers, Producers, PubSubRelation)>>,
    },
}
