use anyhow::{bail, Result};
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use moqt_core::{
    messages::control_messages::subscribe::{FilterType, GroupOrder},
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::modules::pubsub_relation_manager::commands::{
    PubSubRelationCommand, PubSubRelationCommand::*,
};

// Wrapper to encapsulate channel-related operations
pub(crate) struct PubSubRelationManagerWrapper {
    tx: mpsc::Sender<PubSubRelationCommand>,
}

impl PubSubRelationManagerWrapper {
    pub fn new(tx: mpsc::Sender<PubSubRelationCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl PubSubRelationManagerRepository for PubSubRelationManagerWrapper {
    async fn setup_publisher(
        &self,
        max_subscribe_id: u64,
        upstream_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = PubSubRelationCommand::SetupPublisher {
            max_subscribe_id,
            upstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn set_upstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        upstream_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = PubSubRelationCommand::SetUpstreamAnnouncedNamespace {
            track_namespace,
            upstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn set_downstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        downstream_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = PubSubRelationCommand::SetDownstreamAnnouncedNamespace {
            track_namespace,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn set_downstream_subscribed_namespace_prefix(
        &self,
        track_namespace_prefix: Vec<String>,
        downstream_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = PubSubRelationCommand::SetDownstreamSubscribedNamespacePrefix {
            track_namespace_prefix,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn setup_subscriber(
        &self,
        max_subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = PubSubRelationCommand::SetupSubscriber {
            max_subscribe_id,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn is_downstream_subscribe_id_unique(
        &self,
        subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = PubSubRelationCommand::IsDownstreamSubscribeIdUnique {
            subscribe_id,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_unique) => Ok(is_unique),
            Err(err) => bail!(err),
        }
    }
    async fn is_downstream_subscribe_id_less_than_max(
        &self,
        subscribe_id: u64,
        downstream_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = PubSubRelationCommand::IsDownstreamSubscribeIdLessThanMax {
            subscribe_id,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_less) => Ok(is_less),
            Err(err) => bail!(err),
        }
    }
    async fn is_downstream_track_alias_unique(
        &self,
        track_alias: u64,
        downstream_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = PubSubRelationCommand::IsDownstreamTrackAliasUnique {
            track_alias,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_unique) => Ok(is_unique),
            Err(err) => bail!(err),
        }
    }
    async fn is_track_existing(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = PubSubRelationCommand::IsTrackExisting {
            track_namespace,
            track_name,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_existing) => Ok(is_existing),
            Err(err) => bail!(err),
        }
    }
    async fn get_upstream_subscription_by_full_track_name(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<Option<Subscription>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<Subscription>>>();
        let cmd = PubSubRelationCommand::GetUpstreamSubscriptionByFullTrackName {
            track_namespace,
            track_name,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subscription) => Ok(subscription),
            Err(err) => bail!(err),
        }
    }
    async fn get_upstream_subscription_by_ids(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Option<Subscription>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<Subscription>>>();
        let cmd = PubSubRelationCommand::GetUpstreamSubscriptionBySessionIdAndSubscribeId {
            upstream_session_id,
            upstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subscription) => Ok(subscription),
            Err(err) => bail!(err),
        }
    }
    async fn get_downstream_subscription_by_ids(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<Option<Subscription>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<Subscription>>>();
        let cmd = PubSubRelationCommand::GetDownstreamSubscriptionBySessionIdAndSubscribeId {
            downstream_session_id,
            downstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subscription) => Ok(subscription),
            Err(err) => bail!(err),
        }
    }
    async fn get_upstream_session_id(&self, track_namespace: Vec<String>) -> Result<Option<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<usize>>>();
        let cmd = PubSubRelationCommand::GetUpstreamSessionId {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(upstream_session_id) => Ok(upstream_session_id),
            Err(err) => bail!(err),
        }
    }
    async fn get_requesting_downstream_session_ids_and_subscribe_ids(
        &self,
        upstream_subscribe_id: u64,
        upstream_session_id: usize,
    ) -> Result<Option<Vec<(usize, u64)>>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<Vec<(usize, u64)>>>>();
        let cmd = PubSubRelationCommand::GetRequestingDownstreamSessionIdsAndSubscribeIds {
            upstream_subscribe_id,
            upstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(requesting_subscribers) => Ok(requesting_subscribers),
            Err(err) => bail!(err),
        }
    }
    async fn get_upstream_subscribe_id(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
        upstream_session_id: usize,
    ) -> Result<Option<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<u64>>>();
        let cmd = PubSubRelationCommand::GetUpstreamSubscribeId {
            track_namespace,
            track_name,
            upstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subscribe_id) => Ok(subscribe_id),
            Err(err) => bail!(err),
        }
    }
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
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = PubSubRelationCommand::SetDownstreamSubscription {
            downstream_session_id,
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
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
    ) -> Result<(u64, u64)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(u64, u64)>>();
        let cmd = PubSubRelationCommand::SetUpstreamSubscription {
            upstream_session_id,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok((subscribe_id, track_alias)) => Ok((subscribe_id, track_alias)),
            Err(err) => bail!(err),
        }
    }
    async fn set_pubsub_relation(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = SetPubSubRelation {
            upstream_session_id,
            upstream_subscribe_id,
            downstream_session_id,
            downstream_subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn activate_downstream_subscription(
        &self,
        downstream_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = ActivateDownstreamSubscription {
            downstream_session_id,
            subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(activation_occured) => Ok(activation_occured),
            Err(err) => bail!(err),
        }
    }
    async fn activate_upstream_subscription(
        &self,
        upstream_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = ActivateUpstreamSubscription {
            upstream_session_id,
            subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(activation_occured) => Ok(activation_occured),
            Err(err) => bail!(err),
        }
    }
    async fn get_upstream_namespaces_matches_prefix(
        &self,
        track_namespace_prefix: Vec<String>,
    ) -> Result<Vec<Vec<String>>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Vec<Vec<String>>>>();
        let cmd = GetUpstreamNamespacesMatchesPrefix {
            track_namespace_prefix,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(namespaces) => Ok(namespaces),
            Err(err) => bail!(err),
        }
    }
    async fn is_namespace_announced(
        &self,
        track_namespace: Vec<String>,
        downstream_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = IsNamespaceAlreadyAnnounced {
            track_namespace,
            downstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_announced) => Ok(is_announced),
            Err(err) => bail!(err),
        }
    }
    async fn get_downstream_session_ids_by_upstream_namespace(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<Vec<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Vec<usize>>>();
        let cmd = GetDownstreamSessionIdsByUpstreamNamespace {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(session_ids) => Ok(session_ids),
            Err(err) => bail!(err),
        }
    }
    async fn delete_upstream_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        upstream_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = DeleteUpstreamAnnouncedNamespace {
            track_namespace,
            upstream_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(delete_occured) => Ok(delete_occured),
            Err(err) => bail!(err),
        }
    }
    async fn delete_client(&self, session_id: usize) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = DeleteClient {
            session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(delete_occured) => Ok(delete_occured),
            Err(err) => bail!(err),
        }
    }
    async fn delete_pubsub_relation(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = DeletePubSubRelation {
            upstream_session_id,
            upstream_subscribe_id,
            downstream_session_id,
            downstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    async fn delete_upstream_subscription(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = DeleteUpstreamSubscription {
            upstream_session_id,
            upstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    async fn delete_downstream_subscription(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = DeleteDownstreamSubscription {
            downstream_session_id,
            downstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    async fn set_downstream_forwarding_preference(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        forwarding_preference: ForwardingPreference,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = SetDownstreamForwardingPreference {
            downstream_session_id,
            downstream_subscribe_id,
            forwarding_preference,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    async fn set_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        forwarding_preference: ForwardingPreference,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
        let cmd = SetUpstreamForwardingPreference {
            upstream_session_id,
            upstream_subscribe_id,
            forwarding_preference,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    async fn get_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Option<ForwardingPreference>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<ForwardingPreference>>>();
        let cmd = GetUpstreamForwardingPreference {
            upstream_session_id,
            upstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(forwarding_preference) => Ok(forwarding_preference),
            Err(err) => bail!(err),
        }
    }

    async fn get_related_subscribers(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<Vec<(usize, u64)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Vec<(usize, u64)>>>();
        let cmd = PubSubRelationCommand::GetRelatedSubscribers {
            upstream_session_id,
            upstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(related_subscribers) => Ok(related_subscribers),
            Err(err) => bail!(err),
        }
    }

    async fn get_related_publisher(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<(usize, u64)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(usize, u64)>>();
        let cmd = PubSubRelationCommand::GetRelatedPublisher {
            downstream_session_id,
            downstream_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(related_publisher) => Ok(related_publisher),
            Err(err) => bail!(err),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_helper_fn {
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand,
        manager::{Consumers, Producers},
        relation::PubSubRelation,
        wrapper::PubSubRelationManagerWrapper,
    };
    use anyhow::Result;

    use tokio::sync::oneshot;

    pub(crate) async fn get_node_and_relation_clone(
        pubsub_relation_manager: &PubSubRelationManagerWrapper,
    ) -> (Consumers, Producers, PubSubRelation) {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<_>>();
        let cmd = PubSubRelationCommand::GetNodeAndRelationClone { resp: resp_tx };
        pubsub_relation_manager.tx.send(cmd).await.unwrap();

        resp_rx.await.unwrap().unwrap()
    }
}

#[cfg(test)]
mod success {
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager, wrapper::test_helper_fn,
        wrapper::PubSubRelationManagerWrapper,
    };
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::models::subscriptions::{
        nodes::registry::SubscriptionNodeRegistry, Subscription,
    };
    use moqt_core::models::tracks::ForwardingPreference;
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn setup_publisher() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let result = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        assert!(result.is_ok());

        // Check if the publisher is created
        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let length = consumers.len();

        assert_eq!(length, 1);
    }

    #[tokio::test]
    async fn set_upstream_announced_namespace() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let result = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        assert!(result.is_ok());

        // Check if the track_namespace is set
        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        let consumer = consumers.get(&upstream_session_id).unwrap();
        let announced_namespaces = consumer.get_namespaces().unwrap();
        let announced_namespace = announced_namespaces.first().unwrap().to_vec();

        assert_eq!(announced_namespace, track_namespace);
    }

    #[tokio::test]
    async fn set_downstream_announced_namespace() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let result = pubsub_relation_manager
            .set_downstream_announced_namespace(track_namespace.clone(), downstream_session_id)
            .await;
        assert!(result.is_ok());

        // Check if the track_namespace is set
        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        let producer = producers.get(&downstream_session_id).unwrap();
        let announced_namespaces = producer.get_namespaces().unwrap();
        let announced_namespace = announced_namespaces.first().unwrap().to_vec();

        assert_eq!(announced_namespace, track_namespace);
    }

    #[tokio::test]
    async fn set_downstream_subscribed_namespace_prefix() {
        let max_subscribe_id = 10;
        let track_namespace_prefix = Vec::from(["test".to_string(), "test".to_string()]);
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let result = pubsub_relation_manager
            .set_downstream_subscribed_namespace_prefix(
                track_namespace_prefix.clone(),
                downstream_session_id,
            )
            .await;
        assert!(result.is_ok());

        // Check if the track_namespace_prefix is set
        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        let producer = producers.get(&downstream_session_id).unwrap();
        let subscribed_namespace_prefixes = producer.get_namespace_prefixes().unwrap();
        let subscribed_namespace_prefix = subscribed_namespace_prefixes.first().unwrap().to_vec();

        assert_eq!(subscribed_namespace_prefix, track_namespace_prefix);
    }

    #[tokio::test]
    async fn setup_subscriber() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let result = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        assert!(result.is_ok());

        // Check if the subscriber is created
        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let length = producers.len();

        assert_eq!(length, 1);
    }

    #[tokio::test]
    async fn is_downstream_subscribe_id_unique_true() {
        let max_subscribe_id = 10;
        let subscribe_id = 1;
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_subscribe_id_unique(subscribe_id, downstream_session_id)
            .await;

        let is_unique = result.unwrap();
        assert!(is_unique);
    }

    #[tokio::test]
    async fn is_downstream_subscribe_id_unique_false() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let result = pubsub_relation_manager
            .is_downstream_subscribe_id_unique(subscribe_id, downstream_session_id)
            .await;

        let is_unique = result.unwrap();
        assert!(!is_unique);
    }

    #[tokio::test]
    async fn is_downstream_subscribe_id_less_than_max_true() {
        let max_subscribe_id = 10;
        let subscribe_id = 1;
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_subscribe_id_less_than_max(subscribe_id, downstream_session_id)
            .await;

        let is_less = result.unwrap();
        assert!(is_less);
    }

    #[tokio::test]
    async fn is_downstream_subscribe_id_less_than_max_false() {
        let max_subscribe_id = 10;
        let subscribe_id = 11;
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_subscribe_id_less_than_max(subscribe_id, downstream_session_id)
            .await;

        let is_less = result.unwrap();
        assert!(!is_less);
    }

    #[tokio::test]
    async fn is_downstream_track_alias_unique_true() {
        let max_subscribe_id = 10;
        let track_alias = 1;
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_track_alias_unique(track_alias, downstream_session_id)
            .await;

        let is_unique = result.unwrap();
        assert!(is_unique);
    }

    #[tokio::test]
    async fn is_unique_downstream_track_alias_false() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let result = pubsub_relation_manager
            .is_downstream_track_alias_unique(track_alias, downstream_session_id)
            .await;
        assert!(result.is_ok());

        let is_unique = result.unwrap();
        assert!(!is_unique);
    }

    #[tokio::test]
    async fn is_track_existing_exists() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let result = pubsub_relation_manager
            .is_track_existing(track_namespace, track_name)
            .await;
        assert!(result.is_ok());

        let is_existing = result.unwrap();
        assert!(is_existing);
    }

    #[tokio::test]
    async fn is_track_existing_not_exists() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "test_name".to_string();

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let result = pubsub_relation_manager
            .is_track_existing(track_namespace, track_name)
            .await;
        assert!(result.is_ok());

        let is_existing = result.unwrap();
        assert!(!is_existing);
    }

    #[tokio::test]
    async fn get_upstream_subscription_by_full_track_name() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let subscription = pubsub_relation_manager
            .get_upstream_subscription_by_full_track_name(
                track_namespace.clone(),
                track_name.clone(),
            )
            .await
            .unwrap();

        let forwarding_preference = None;
        let expected_subscription = Subscription::new(
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            forwarding_preference,
        );

        assert_eq!(subscription, Some(expected_subscription));
    }

    #[tokio::test]
    async fn get_upstream_subscription_by_ids() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let upstream_subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let subscription = pubsub_relation_manager
            .get_upstream_subscription_by_ids(upstream_session_id, upstream_subscribe_id)
            .await
            .unwrap();

        let forwarding_preference = None;
        let expected_subscription = Subscription::new(
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            forwarding_preference,
        );

        assert_eq!(subscription, Some(expected_subscription));
    }

    #[tokio::test]
    async fn get_downstream_subscription_by_ids() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let downstream_subscribe_id = 1;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                downstream_subscribe_id,
                track_alias,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let subscription = pubsub_relation_manager
            .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
            .await
            .unwrap();

        let forwarding_preference = None;
        let expected_subscription = Subscription::new(
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            forwarding_preference,
        );

        assert_eq!(subscription, Some(expected_subscription));
    }

    #[tokio::test]
    async fn get_upstream_session_id() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let session_id = pubsub_relation_manager
            .get_upstream_session_id(track_namespace.clone())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(session_id, upstream_session_id);
    }

    #[tokio::test]
    async fn get_requesting_downstream_session_ids_and_subscribe_ids() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let downstream_session_ids = [2, 3];
        let downstream_subscribe_ids = [4, 5];
        let downstream_track_aliases = [6, 7];
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        for i in [0, 1] {
            let _ = pubsub_relation_manager
                .setup_subscriber(max_subscribe_id, downstream_session_ids[i])
                .await;
            let _ = pubsub_relation_manager
                .set_downstream_subscription(
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                    downstream_track_aliases[i],
                    track_namespace.clone(),
                    track_name.clone(),
                    subscriber_priority,
                    group_order,
                    filter_type,
                    start_group,
                    start_object,
                    end_group,
                    end_object,
                )
                .await;
            let _ = pubsub_relation_manager
                .set_pubsub_relation(
                    upstream_session_id,
                    upstream_subscribe_id,
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                )
                .await;
        }

        let list = pubsub_relation_manager
            .get_requesting_downstream_session_ids_and_subscribe_ids(
                upstream_subscribe_id,
                upstream_session_id,
            )
            .await
            .unwrap()
            .unwrap();

        let expected_list = vec![
            (downstream_session_ids[0], downstream_subscribe_ids[0]),
            (downstream_session_ids[1], downstream_subscribe_ids[1]),
        ];

        assert_eq!(list, expected_list);
    }

    #[tokio::test]
    async fn get_upstream_subscribe_id() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (expected_upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        let upstream_subscribe_id = pubsub_relation_manager
            .get_upstream_subscribe_id(track_namespace, track_name, upstream_session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(upstream_subscribe_id, expected_upstream_subscribe_id);
    }

    #[tokio::test]
    async fn set_downstream_subscription() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let result = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        assert!(result.is_ok());

        // Assert that the subscription is set
        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let producer = producers.get(&downstream_session_id).unwrap();
        let subscription = producer.get_subscription(subscribe_id).unwrap().unwrap();

        let expected_subscription = Subscription::new(
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            None,
        );

        assert_eq!(subscription, expected_subscription);
    }

    #[tokio::test]
    async fn set_upstream_subscription() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let result = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        assert!(result.is_ok());

        let (upstream_subscribe_id, upstream_track_alias) = result.unwrap();

        // Assert that the subscription is set
        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let consumer = consumers.get(&upstream_session_id).unwrap();
        let subscription = consumer
            .get_subscription(upstream_subscribe_id)
            .unwrap()
            .unwrap();

        let expected_subscription = Subscription::new(
            upstream_track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            None,
        );

        assert_eq!(subscription, expected_subscription);
    }

    #[tokio::test]
    async fn set_pubsub_relation() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let downstream_session_ids = [2, 3];
        let downstream_subscribe_ids = [4, 5];
        let downstream_track_aliases = [6, 7];
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        //   pub 1 <- sub 2, 3
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        for i in [0, 1] {
            let _ = pubsub_relation_manager
                .setup_subscriber(max_subscribe_id, downstream_session_ids[i])
                .await;
            let _ = pubsub_relation_manager
                .set_downstream_subscription(
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                    downstream_track_aliases[i],
                    track_namespace.clone(),
                    track_name.clone(),
                    subscriber_priority,
                    group_order,
                    filter_type,
                    start_group,
                    start_object,
                    end_group,
                    end_object,
                )
                .await;
            let result = pubsub_relation_manager
                .set_pubsub_relation(
                    upstream_session_id,
                    upstream_subscribe_id,
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                )
                .await;

            assert!(result.is_ok());
        }

        // Assert that the relation is registered
        let (_, _, pubsub_relation) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        let subscriber = pubsub_relation
            .get_subscribers(upstream_session_id, upstream_subscribe_id)
            .unwrap()
            .to_vec();

        let expected_subscriber = vec![
            (downstream_session_ids[0], downstream_subscribe_ids[0]),
            (downstream_session_ids[1], downstream_subscribe_ids[1]),
        ];

        assert_eq!(subscriber, expected_subscriber);
    }

    #[tokio::test]
    async fn activate_downstream_subscription() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let activate_occured = pubsub_relation_manager
            .activate_downstream_subscription(downstream_session_id, subscribe_id)
            .await
            .unwrap();

        assert!(activate_occured);

        // Assert that the subscription is active
        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let producer = producers.get(&downstream_session_id).unwrap();
        let subscription = producer.get_subscription(subscribe_id).unwrap().unwrap();

        assert!(subscription.is_active());
    }

    #[tokio::test]
    async fn activate_upstream_subscription() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        let activate_occured = pubsub_relation_manager
            .activate_upstream_subscription(upstream_session_id, upstream_subscribe_id)
            .await
            .unwrap();

        assert!(activate_occured);

        // Assert that the subscription is active
        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let consumer = consumers.get(&upstream_session_id).unwrap();
        let subscription = consumer
            .get_subscription(upstream_subscribe_id)
            .unwrap()
            .unwrap();

        assert!(subscription.is_active());
    }

    #[tokio::test]
    async fn get_upstream_namespaces_matches_prefix_exist() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let namespaces = pubsub_relation_manager
            .get_upstream_namespaces_matches_prefix(track_namespace_prefix)
            .await
            .unwrap();

        assert_eq!(namespaces, vec![track_namespace]);
    }

    #[tokio::test]
    async fn get_upstream_namespaces_matches_prefix_not_exist() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let track_namespace_prefix = Vec::from(["aa".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let namespaces = pubsub_relation_manager
            .get_upstream_namespaces_matches_prefix(track_namespace_prefix)
            .await
            .unwrap();

        let expected_namespaces: Vec<Vec<String>> = vec![];

        assert_eq!(namespaces, expected_namespaces);
    }

    #[tokio::test]
    async fn is_namespace_announced_exist() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_announced_namespace(track_namespace.clone(), downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_namespace_announced(track_namespace.clone(), downstream_session_id)
            .await;

        let is_announced = result.unwrap();
        assert!(is_announced);
    }

    #[tokio::test]
    async fn is_namespace_announced_not_exist() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_namespace_announced(track_namespace.clone(), downstream_session_id)
            .await;

        let is_announced = result.unwrap();
        assert!(!is_announced);
    }

    #[tokio::test]
    async fn get_downstream_session_ids_by_upstream_namespace() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let downstream_session_ids = [2, 3];
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefixes = Vec::from([
            Vec::from(["aaa".to_string()]),
            Vec::from(["bbb".to_string()]),
        ]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        //   pub 1 <- sub 2, 3
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        for i in [0, 1] {
            let _ = pubsub_relation_manager
                .setup_subscriber(max_subscribe_id, downstream_session_ids[i])
                .await;

            let _ = pubsub_relation_manager
                .set_downstream_subscribed_namespace_prefix(
                    track_namespace_prefixes[i].clone(),
                    downstream_session_ids[i],
                )
                .await;
        }

        let result = pubsub_relation_manager
            .get_downstream_session_ids_by_upstream_namespace(track_namespace)
            .await;

        assert!(result.is_ok());

        let expected_downstream_session_ids = vec![downstream_session_ids[0]];

        assert_eq!(result.unwrap(), expected_downstream_session_ids);
    }

    #[tokio::test]
    async fn delete_upstream_announced_namespace() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .delete_upstream_announced_namespace(track_namespace, upstream_session_id)
            .await;
        assert!(result.is_ok());

        let delete_occured = result.unwrap();
        assert!(delete_occured);

        // Assert that the announced namespace is deleted
        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let consumer = consumers.get(&upstream_session_id).unwrap();
        let announced_namespaces = consumer.get_namespaces().unwrap().to_vec();

        assert!(announced_namespaces.is_empty());
    }

    #[tokio::test]
    async fn delete_upstream_announced_namespace_not_exists() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let result = pubsub_relation_manager
            .delete_upstream_announced_namespace(track_namespace, upstream_session_id)
            .await;
        assert!(result.is_ok());

        let delete_occured = result.unwrap();
        assert!(!delete_occured);
    }

    #[tokio::test]
    async fn delete_client() {
        let max_subscribe_id = 10;
        let track_namespaces = [
            Vec::from(["test1".to_string(), "test1".to_string()]),
            Vec::from(["test2".to_string(), "test2".to_string()]),
        ];
        let upstream_session_ids = [1, 2];
        let mut upstream_subscribe_ids = vec![];
        let downstream_session_ids = [2, 3, 4];
        let downstream_subscribe_ids = [2, 3, 4];
        let downstream_track_aliases = [2, 3, 4];
        let track_name = "test_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        // Register:
        //   pub 1 <- sub 2, 3, 4
        //   pub 2 <- sub 3, 4
        for i in [0, 1] {
            // for pub 1, 2
            let _ = pubsub_relation_manager
                .setup_publisher(max_subscribe_id, upstream_session_ids[i])
                .await;
            let _ = pubsub_relation_manager
                .set_upstream_announced_namespace(
                    track_namespaces[i].clone(),
                    upstream_session_ids[i],
                )
                .await;
            let (upstream_subscribe_id, _) = pubsub_relation_manager
                .set_upstream_subscription(
                    upstream_session_ids[i],
                    track_namespaces[i].clone(),
                    track_name.clone(),
                    subscriber_priority,
                    group_order,
                    filter_type,
                    start_group,
                    start_object,
                    end_group,
                    end_object,
                )
                .await
                .unwrap();
            upstream_subscribe_ids.push(upstream_subscribe_id);
        }

        for j in [0, 1, 2] {
            // for sub 2, 3, 4
            let _ = pubsub_relation_manager
                .setup_subscriber(max_subscribe_id, downstream_session_ids[j])
                .await;
        }

        // for sub 2
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_ids[0],
                downstream_subscribe_ids[0],
                downstream_track_aliases[0],
                track_namespaces[0].clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        for i in [0, 1] {
            // for pub 1, 2
            for j in [1, 2] {
                // for sub 3, 4
                let _ = pubsub_relation_manager
                    .set_downstream_subscription(
                        downstream_session_ids[j],
                        downstream_subscribe_ids[j],
                        downstream_track_aliases[j],
                        track_namespaces[i].clone(),
                        track_name.clone(),
                        subscriber_priority,
                        group_order,
                        filter_type,
                        start_group,
                        start_object,
                        end_group,
                        end_object,
                    )
                    .await;

                let _ = pubsub_relation_manager
                    .set_pubsub_relation(
                        upstream_session_ids[i],
                        upstream_subscribe_ids[i],
                        downstream_session_ids[j],
                        downstream_subscribe_ids[j],
                    )
                    .await;
                let _ = pubsub_relation_manager
                    .activate_downstream_subscription(
                        downstream_session_ids[j],
                        downstream_subscribe_ids[j],
                    )
                    .await;

                let _ = pubsub_relation_manager
                    .activate_upstream_subscription(
                        upstream_session_ids[i],
                        upstream_subscribe_ids[i],
                    )
                    .await;
            }
        }

        // for pub 1 and sub 2
        let _ = pubsub_relation_manager
            .set_pubsub_relation(
                upstream_session_ids[0],
                upstream_subscribe_ids[0],
                downstream_session_ids[0],
                downstream_subscribe_ids[0],
            )
            .await;
        let _ = pubsub_relation_manager
            .activate_downstream_subscription(
                downstream_session_ids[0],
                downstream_subscribe_ids[0],
            )
            .await;

        let _ = pubsub_relation_manager
            .activate_upstream_subscription(upstream_session_ids[0], upstream_subscribe_ids[0])
            .await;

        // Delete: pub 2, sub 2
        // Remain: pub 1 <- sub 3, 4
        let result = pubsub_relation_manager
            .delete_client(downstream_session_ids[0])
            .await;
        assert!(result.is_ok());

        let delete_occured = result.unwrap();
        assert!(delete_occured);

        let (consumers, producers, pubsub_relation) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        // Assert that sub 2 is deleted
        // Remain: sub 3, 4
        let sub2 = producers.get(&downstream_session_ids[0]);
        assert!(sub2.is_none());

        let sub3 = producers.get(&downstream_session_ids[1]);
        assert!(sub3.is_some());

        let sub4 = producers.get(&downstream_session_ids[2]);
        assert!(sub4.is_some());

        // Assert that pub 2 is deleted
        // Remain: pub 1
        let pub1 = consumers.get(&upstream_session_ids[1]);
        assert!(pub1.is_none());

        let pub2 = consumers.get(&upstream_session_ids[0]);
        assert!(pub2.is_some());

        // Assert that the relation is deleted
        // Remain: pub 1 <- sub 3, 4
        let pub1_relation =
            pubsub_relation.get_subscribers(upstream_session_ids[0], upstream_subscribe_ids[0]);
        assert!(pub1_relation.is_some());

        let pub2_relation =
            pubsub_relation.get_subscribers(upstream_session_ids[1], upstream_subscribe_ids[1]);
        assert!(pub2_relation.is_none());
    }

    #[tokio::test]
    async fn delete_client_not_exists() {
        let session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let result = pubsub_relation_manager.delete_client(session_id).await;
        assert!(result.is_ok());

        let delete_occured = result.unwrap();
        assert!(!delete_occured);
    }

    #[tokio::test]
    async fn delete_pubsub_relation() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let downstream_session_id = 2;
        let downstream_subscribe_id = 3;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                max_subscribe_id,
                0,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();
        let _ = pubsub_relation_manager
            .set_pubsub_relation(
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
            )
            .await;

        let result = pubsub_relation_manager
            .delete_pubsub_relation(
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
            )
            .await;

        assert!(result.is_ok());

        let (_, _, pubsub_relation) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        let relation = pubsub_relation
            .get_subscribers(upstream_session_id, upstream_subscribe_id)
            .unwrap();

        assert!(relation.is_empty());
    }

    #[tokio::test]
    async fn delete_upstream_subscription() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        let result = pubsub_relation_manager
            .delete_upstream_subscription(upstream_session_id, upstream_subscribe_id)
            .await;
        assert!(result.is_ok());

        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let consumer = consumers.get(&upstream_session_id).unwrap();
        let subscription = consumer.get_subscription(upstream_subscribe_id).unwrap();

        assert!(subscription.is_none());
    }

    #[tokio::test]
    async fn delete_downstream_subscription() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let result = pubsub_relation_manager
            .delete_downstream_subscription(downstream_session_id, subscribe_id)
            .await;
        assert!(result.is_ok());

        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let producer = producers.get(&downstream_session_id).unwrap();
        let subscription = producer.get_subscription(subscribe_id).unwrap();

        assert!(subscription.is_none());
    }

    #[tokio::test]
    async fn set_upstream_forwarding_preference() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let forwarding_preference = ForwardingPreference::Track;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        let result = pubsub_relation_manager
            .set_upstream_forwarding_preference(
                upstream_session_id,
                upstream_subscribe_id,
                forwarding_preference.clone(),
            )
            .await;
        assert!(result.is_ok());

        // Assert that the forwarding preference is set
        let (consumers, _, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let consumer = consumers.get(&upstream_session_id).unwrap();
        let subscription = consumer
            .get_subscription(upstream_subscribe_id)
            .unwrap()
            .unwrap();

        let result_forwarding_preference = subscription.get_forwarding_preference().unwrap();

        assert_eq!(result_forwarding_preference, forwarding_preference);
    }

    #[tokio::test]
    async fn get_upstream_forwarding_preference() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let forwarding_preference = ForwardingPreference::Track;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();
        let _ = pubsub_relation_manager
            .set_upstream_forwarding_preference(
                upstream_session_id,
                upstream_subscribe_id,
                forwarding_preference.clone(),
            )
            .await;

        let result = pubsub_relation_manager
            .get_upstream_forwarding_preference(upstream_session_id, upstream_subscribe_id)
            .await;
        assert!(result.is_ok());

        let result_forwarding_preference = result.unwrap().unwrap();

        assert_eq!(result_forwarding_preference, forwarding_preference);
    }

    #[tokio::test]
    async fn set_downstream_forwarding_preference() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let forwarding_preference = ForwardingPreference::Subgroup;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        let result = pubsub_relation_manager
            .set_downstream_forwarding_preference(
                downstream_session_id,
                subscribe_id,
                forwarding_preference.clone(),
            )
            .await;
        assert!(result.is_ok());

        // Assert that the forwarding preference is set
        let (_, producers, _) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;
        let producer = producers.get(&downstream_session_id).unwrap();
        let subscription = producer.get_subscription(subscribe_id).unwrap().unwrap();

        let result_forwarding_preference = subscription.get_forwarding_preference().unwrap();

        assert_eq!(result_forwarding_preference, forwarding_preference);
    }

    #[tokio::test]
    async fn get_related_subscribers() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let downstream_session_ids = [2, 3];
        let downstream_subscribe_ids = [4, 5];
        let downstream_track_aliases = [6, 7];
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        //   pub 1 <- sub 2, 3
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        for i in [0, 1] {
            let _ = pubsub_relation_manager
                .setup_subscriber(max_subscribe_id, downstream_session_ids[i])
                .await;
            let _ = pubsub_relation_manager
                .set_downstream_subscription(
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                    downstream_track_aliases[i],
                    track_namespace.clone(),
                    track_name.clone(),
                    subscriber_priority,
                    group_order,
                    filter_type,
                    start_group,
                    start_object,
                    end_group,
                    end_object,
                )
                .await;
            let _ = pubsub_relation_manager
                .set_pubsub_relation(
                    upstream_session_id,
                    upstream_subscribe_id,
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                )
                .await;

            let _ = pubsub_relation_manager
                .activate_downstream_subscription(
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                )
                .await;

            let _ = pubsub_relation_manager
                .activate_upstream_subscription(upstream_session_id, upstream_subscribe_id)
                .await;
        }

        let related_subscribers = pubsub_relation_manager
            .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
            .await
            .unwrap();

        let expected_related_subscribers = vec![
            (downstream_session_ids[0], downstream_subscribe_ids[0]),
            (downstream_session_ids[1], downstream_subscribe_ids[1]),
        ];

        assert_eq!(related_subscribers, expected_related_subscribers);
    }

    #[tokio::test]
    async fn get_related_publisher() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let downstream_session_ids = [2, 3];
        let downstream_subscribe_ids = [4, 5];
        let downstream_track_aliases = [6, 7];
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        //   pub 1 <- sub 2, 3
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await
            .unwrap();

        for i in [0, 1] {
            let _ = pubsub_relation_manager
                .setup_subscriber(max_subscribe_id, downstream_session_ids[i])
                .await;
            let _ = pubsub_relation_manager
                .set_downstream_subscription(
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                    downstream_track_aliases[i],
                    track_namespace.clone(),
                    track_name.clone(),
                    subscriber_priority,
                    group_order,
                    filter_type,
                    start_group,
                    start_object,
                    end_group,
                    end_object,
                )
                .await;
            let _ = pubsub_relation_manager
                .set_pubsub_relation(
                    upstream_session_id,
                    upstream_subscribe_id,
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                )
                .await;
            let _ = pubsub_relation_manager
                .activate_downstream_subscription(
                    downstream_session_ids[i],
                    downstream_subscribe_ids[i],
                )
                .await;
            let _ = pubsub_relation_manager
                .activate_upstream_subscription(upstream_session_id, upstream_subscribe_id)
                .await;
        }

        let related_publisher = pubsub_relation_manager
            .get_related_publisher(downstream_session_ids[0], downstream_subscribe_ids[0])
            .await
            .unwrap();

        let expected_related_publisher = (upstream_session_id, upstream_subscribe_id);

        assert_eq!(related_publisher, expected_related_publisher);
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn setup_publisher_already_exist() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        // Register the same publisher
        let result = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_upstream_announced_namespace_already_exist() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        // Register the same track namespace
        let result = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn setup_subscriber_already_exist() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Register the same subscriber
        let result = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn is_downstream_subscribe_id_subscriber_unique_not_found() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let downstream_subscribe_id = 0;
        let invalid_downstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_subscribe_id_unique(
                downstream_subscribe_id,
                invalid_downstream_session_id,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn is_downstream_subscribe_id_subscriber_less_than_max_not_found() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let downstream_subscribe_id = 0;
        let invalid_downstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_subscribe_id_less_than_max(
                downstream_subscribe_id,
                invalid_downstream_session_id,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn is_downstream_track_alias_unique_subscriber_not_found() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let downstream_track_alias = 0;
        let invalid_downstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .is_downstream_track_alias_unique(downstream_track_alias, invalid_downstream_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_requesting_downstream_session_ids_and_subscribe_ids_publisher_not_found() {
        let upstream_session_id = 0;
        let upstream_subscribe_id = 0;
        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let result = pubsub_relation_manager
            .get_requesting_downstream_session_ids_and_subscribe_ids(
                upstream_subscribe_id,
                upstream_session_id,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_upstream_subscribe_id_publisher_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let invalid_upstream_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let result = pubsub_relation_manager
            .get_upstream_subscribe_id(track_namespace, track_name, invalid_upstream_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_downstream_subscription_subscriber_not_found() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let invalid_downstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .set_downstream_subscription(
                invalid_downstream_session_id,
                subscribe_id,
                track_alias,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_upstream_subscription_publisher_not_found() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let invalid_upstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .set_upstream_subscription(
                invalid_upstream_session_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn activate_downstream_subscription_subscriber_not_found() {
        let max_subscribe_id = 10;
        let downstream_session_id = 1;
        let subscribe_id = 0;
        let invalid_downstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .activate_downstream_subscription(invalid_downstream_session_id, subscribe_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn activate_upstream_subscription_publisher_not_found() {
        let max_subscribe_id = 10;
        let upstream_session_id = 1;
        let upstream_subscribe_id = 0;
        let invalid_upstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .activate_upstream_subscription(invalid_upstream_session_id, upstream_subscribe_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_upstream_announced_namespace_publisher_not_found() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let upstream_session_id = 1;
        let invalid_upstream_session_id = 2;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_rx).await });

        let pubsub_relation_manager = PubSubRelationManagerWrapper::new(track_tx.clone());
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let result = pubsub_relation_manager
            .delete_upstream_announced_namespace(track_namespace, invalid_upstream_session_id)
            .await;

        assert!(result.is_err());
    }
}
