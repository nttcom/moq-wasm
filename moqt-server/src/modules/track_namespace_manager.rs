use anyhow::{bail, Result};
use async_trait::async_trait;
use moqt_core::messages::control_messages::subscribe::{self, FilterType, GroupOrder};
use moqt_core::subscription_models::subscription_nodes::SubscriptionNodeRegistory;
use moqt_core::subscription_models::subscription_nodes::{Consumer, Producer};
use moqt_core::subscription_models::subscriptions::Subscription;
use moqt_core::TrackNamespaceManagerRepository;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use TrackCommand::*;

type SubscriberSessionId = usize;
type PublisherSessionId = usize;
type PublishedSubscribeId = u64;
type SubscribedSubscribeId = u64;
type TrackName = String;
type TrackNamespace = Vec<String>;
struct PubSubRelation {
    records: HashMap<
        (PublisherSessionId, PublishedSubscribeId),
        Vec<(SubscriberSessionId, SubscribedSubscribeId)>,
    >,
}

impl PubSubRelation {
    fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    fn add_relation(
        &mut self,
        publisher_session_id: PublisherSessionId,
        published_subscribe_id: PublishedSubscribeId,
        subscriber_session_id: SubscriberSessionId,
        subscribed_subscribe_id: SubscribedSubscribeId,
    ) {
        let key = (publisher_session_id, published_subscribe_id);
        let value = (subscriber_session_id, subscribed_subscribe_id);

        match self.records.get_mut(&key) {
            // If the key exists, add the value to the existing vector
            Some(subscribers) => {
                subscribers.push(value);
            }
            // If the key does not exist, create a new vector and insert the value
            None => {
                self.records.insert(key, vec![value]);
            }
        }
    }

    fn get_subscribers(
        &self,
        publisher_session_id: PublisherSessionId,
        published_subscribe_id: PublishedSubscribeId,
    ) -> Option<&Vec<(SubscriberSessionId, SubscribedSubscribeId)>> {
        let key = (publisher_session_id, published_subscribe_id);
        self.records.get(&key)
    }

    fn delete_relarion(
        &mut self,
        publisher_session_id: PublisherSessionId,
        published_subscribe_id: PublishedSubscribeId,
        subscriber_session_id: SubscriberSessionId,
        subscribed_subscribe_id: SubscribedSubscribeId,
    ) {
        let key = (publisher_session_id, published_subscribe_id);
        if let Some(subscribers) = self.records.get_mut(&key) {
            subscribers.retain(|(session_id, subscribe_id)| {
                *session_id != subscriber_session_id && *subscribe_id != subscribed_subscribe_id
            });
        }
    }
}

// [Original Publisher: (Producer) ] -> [Relay: (Consumer) - <PubSubRelation> - (Producer) ] -> [End Subscriber: (Consumer)]

// Called as a separate thread
pub(crate) async fn track_namespace_manager(rx: &mut mpsc::Receiver<TrackCommand>) {
    tracing::trace!("track_namespace_manager start");

    let mut consumers: HashMap<PublisherSessionId, Consumer> = HashMap::new();
    let mut producers: HashMap<SubscriberSessionId, Producer> = HashMap::new();
    let mut pubsub_relation = PubSubRelation::new();

    while let Some(cmd) = rx.recv().await {
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            SetupPublisher {
                max_subscribe_id,
                publisher_session_id,
                resp,
            } => {
                consumers.insert(publisher_session_id, Consumer::new(max_subscribe_id));
                resp.send(true).unwrap();
            }
            SetPublisherAnnouncedNamespace {
                track_namespace,
                publisher_session_id,
                resp,
            } => {
                let consumer = consumers.get_mut(&publisher_session_id).unwrap();
                match consumer.set_namespace(track_namespace) {
                    Ok(_) => {
                        resp.send(true).unwrap();
                    }
                    Err(err) => {
                        tracing::error!("set_namespace: err: {:?}", err.to_string());
                        resp.send(false).unwrap();
                    }
                }
            }
            SetupSubscriber {
                max_subscribe_id,
                subscriber_session_id,
                resp,
            } => {
                producers.insert(subscriber_session_id, Producer::new(max_subscribe_id));
                resp.send(true).unwrap();
            }
            IsValidSubscriberSubscribeId {
                subscribe_id,
                subscriber_session_id,
                resp,
            } => {
                let producer = producers.get(&subscriber_session_id).unwrap();

                if producer.is_within_max_subscribe_id(subscribe_id)
                    && producer.is_subscribe_id_unique(subscribe_id)
                {
                    resp.send(true).unwrap();
                } else {
                    resp.send(false).unwrap();
                }
            }
            IsValidSubscriberTrackAlias {
                track_alias,
                subscriber_session_id,
                resp,
            } => {
                let producer = producers.get(&subscriber_session_id).unwrap();
                let is_valid = producer.is_track_alias_unique(track_alias);
                resp.send(is_valid).unwrap();
            }
            GetPublisherSessionId {
                track_namespace,
                resp,
            } => {
                // Find the publisher that has the track namespace from all consumers
                let publisher_session_id = consumers
                    .iter()
                    .find(|(_, consumer)| consumer.has_namespace(track_namespace.clone()))
                    .map(|(session_id, _)| *session_id);
                resp.send(publisher_session_id).unwrap();
            }
            GetRequestingSubscriberSessionIdsAndSubscribeIds {
                published_subscribe_id,
                publisher_session_id,
                resp,
            } => {
                let subscribers =
                    pubsub_relation.get_subscribers(publisher_session_id, published_subscribe_id);

                resp.send(subscribers.cloned()).unwrap();
            }
            GetPublisherSubscribeId {
                track_namespace,
                track_name,
                publisher_session_id,
                resp,
            } => {
                let consumer = consumers.get_mut(&publisher_session_id).unwrap();
                let result = consumer.get_subscribe_id(track_namespace, track_name);

                match result {
                    Ok(subscribe_id) => resp.send(subscribe_id).unwrap(),
                    Err(_) => resp.send(None).unwrap(),
                }
            }
            IsTrackExisting {
                track_namespace,
                track_name,
                resp,
            } => {
                let consumer = consumers.iter().find(|(_, consumer)| {
                    consumer.has_track(track_namespace.clone(), track_name.clone())
                });
                let is_existing = consumer.is_some();
                resp.send(is_existing).unwrap();
            }
            GetPublisherSubscription {
                track_namespace,
                track_name,
                resp,
            } => {
                let consumer = consumers.iter().find(|(_, consumer)| {
                    consumer.has_track(track_namespace.clone(), track_name.clone())
                });
                let result = consumer
                    .map(|(_, consumer)| {
                        consumer.get_subscription_by_full_track_name(track_namespace, track_name)
                    })
                    .unwrap();

                match result {
                    Ok(subscription) => resp.send(subscription).unwrap(),
                    Err(_) => resp.send(None).unwrap(),
                }
            }
            SetSubscriberSubscription {
                subscriber_session_id,
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
                resp,
            } => {
                let producer = producers.get_mut(&subscriber_session_id).unwrap();
                match producer.set_subscription(
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
                ) {
                    Ok(_) => resp.send(true).unwrap(),
                    Err(err) => {
                        tracing::error!("set_subscriber_subscription: err: {:?}", err.to_string());
                        resp.send(false).unwrap();
                    }
                }
            }
            SetPublisherSubscription {
                publisher_session_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
                resp,
            } => {
                let consumer = consumers.get_mut(&publisher_session_id).unwrap();
                let (subscribe_id, track_alias) =
                    consumer.find_unused_subscribe_id_and_track_alias().unwrap();
                match consumer.set_subscription(
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
                ) {
                    Ok(_) => resp.send((subscribe_id, track_alias)).unwrap(),
                    Err(err) => {
                        tracing::error!("set_publisher_subscription: err: {:?}", err.to_string());
                        resp.send((0, 0)).unwrap();
                        continue;
                    }
                };
            }
            RegisterPubSupRelation {
                publisher_session_id,
                published_subscribe_id,
                subscriber_session_id,
                subscribed_subscribe_id,
                resp,
            } => {
                pubsub_relation.add_relation(
                    publisher_session_id,
                    published_subscribe_id,
                    subscriber_session_id,
                    subscribed_subscribe_id,
                );

                resp.send(true).unwrap();
            }
            ActivateSubscriberSubscription {
                subscriber_session_id,
                subscribe_id,
                resp,
            } => {
                let producer = producers.get_mut(&subscriber_session_id).unwrap();
                producer.activate_subscription(subscribe_id);
                resp.send(true).unwrap();
            }
            ActivatePublisherSubscription {
                publisher_session_id,
                subscribe_id,
                resp,
            } => {
                let consumer = consumers.get_mut(&publisher_session_id).unwrap();
                consumer.activate_subscription(subscribe_id);
                resp.send(true).unwrap();
            }
            DeletePublisherAnnouncedNamespace {
                track_namespace,
                publisher_session_id,
                resp,
            } => {
                let consumer = consumers.get_mut(&publisher_session_id).unwrap();
                consumer.delete_namespace(track_namespace);
                resp.send(true).unwrap();
            }
            DeleteClient { session_id, resp } => {
                // Delete as a publisher
                consumers.remove(&session_id);
                pubsub_relation.records.retain(|key, _| key.0 != session_id);

                // Delete as a subscriber
                producers.remove(&session_id);
                pubsub_relation
                    .records
                    .iter_mut()
                    .for_each(|(_, subscribers)| {
                        subscribers.retain(|(subscriber_session_id, _)| {
                            *subscriber_session_id != session_id
                        });
                    });

                resp.send(true).unwrap();
            }
        }
    }

    tracing::trace!("track_namespace_manager end");
}

#[derive(Debug)]
pub(crate) enum TrackCommand {
    SetupPublisher {
        max_subscribe_id: u64,
        publisher_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    SetPublisherAnnouncedNamespace {
        track_namespace: Vec<String>,
        publisher_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    SetupSubscriber {
        max_subscribe_id: u64,
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    IsValidSubscriberSubscribeId {
        subscribe_id: u64,
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    IsValidSubscriberTrackAlias {
        track_alias: u64,
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    IsTrackExisting {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    GetPublisherSubscription {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<Option<Subscription>>,
    },
    GetPublisherSessionId {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<Option<usize>>,
    },
    GetRequestingSubscriberSessionIdsAndSubscribeIds {
        published_subscribe_id: u64,
        publisher_session_id: usize,
        resp: oneshot::Sender<Option<Vec<(usize, u64)>>>,
    },
    GetPublisherSubscribeId {
        track_namespace: Vec<String>,
        track_name: String,
        publisher_session_id: usize,
        resp: oneshot::Sender<Option<u64>>,
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
        resp: oneshot::Sender<bool>,
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
        resp: oneshot::Sender<(u64, u64)>,
    },
    RegisterPubSupRelation {
        publisher_session_id: usize,
        published_subscribe_id: u64,
        subscriber_session_id: usize,
        subscribed_subscribe_id: u64,
        resp: oneshot::Sender<bool>,
    },
    ActivateSubscriberSubscription {
        subscriber_session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<bool>,
    },
    ActivatePublisherSubscription {
        publisher_session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<bool>,
    },
    DeletePublisherAnnouncedNamespace {
        track_namespace: Vec<String>,
        publisher_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    DeleteClient {
        session_id: usize,
        resp: oneshot::Sender<bool>,
    },
}

// Wrapper to encapsulate channel-related operations
pub(crate) struct TrackNamespaceManager {
    tx: mpsc::Sender<TrackCommand>,
}

impl TrackNamespaceManager {
    pub fn new(tx: mpsc::Sender<TrackCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl TrackNamespaceManagerRepository for TrackNamespaceManager {
    async fn setup_publisher(
        &self,
        max_subscribe_id: u64,
        publisher_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetupPublisher {
            max_subscribe_id,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("already exist"),
        }
    }
    async fn set_publisher_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetPublisherAnnouncedNamespace {
            track_namespace,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("already exist"),
        }
    }
    async fn setup_subscriber(
        &self,
        max_subscribe_id: u64,
        subscriber_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetupSubscriber {
            max_subscribe_id,
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("already exist"),
        }
    }
    async fn is_valid_subscriber_subscribe_id(
        &self,
        subscribe_id: u64,
        subscriber_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = TrackCommand::IsValidSubscriberSubscribeId {
            subscribe_id,
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        return Ok(result);
    }
    async fn is_valid_subscriber_track_alias(
        &self,
        track_alias: u64,
        subscriber_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = TrackCommand::IsValidSubscriberTrackAlias {
            track_alias,
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        return Ok(result);
    }
    async fn is_track_existing(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = TrackCommand::IsTrackExisting {
            track_namespace,
            track_name,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        return Ok(result);
    }
    async fn get_publisher_subscription_by_full_track_name(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<Option<Subscription>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<Subscription>>();
        let cmd = TrackCommand::GetPublisherSubscription {
            track_namespace,
            track_name,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await;

        match result {
            Ok(subscription) => Ok(subscription),
            Err(_) => bail!("not found"),
        }
    }
    async fn get_publisher_session_id(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<Option<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<usize>>();
        let cmd = TrackCommand::GetPublisherSessionId {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let session_id = resp_rx.await.unwrap();

        return Ok(session_id);
    }
    async fn get_requesting_subscriber_session_ids_and_subscribe_ids(
        &self,
        published_subscribe_id: u64,
        publisher_session_id: usize,
    ) -> Result<Option<Vec<(usize, u64)>>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<Vec<(usize, u64)>>>();
        let cmd = TrackCommand::GetRequestingSubscriberSessionIdsAndSubscribeIds {
            published_subscribe_id,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        return Ok(result);
    }
    async fn get_publisher_subscribe_id(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
        publisher_session_id: usize,
    ) -> Result<Option<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<u64>>();
        let cmd = TrackCommand::GetPublisherSubscribeId {
            track_namespace,
            track_name,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let subscribe_id = resp_rx.await.unwrap();

        return Ok(subscribe_id);
    }
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
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = TrackCommand::SetSubscriberSubscription {
            subscriber_session_id,
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
            true => Ok(()),
            false => bail!("already exist"),
        }
    }
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
    ) -> Result<(u64, u64)> {
        let (resp_tx, resp_rx) = oneshot::channel::<(u64, u64)>();
        let cmd = TrackCommand::SetPublisherSubscription {
            publisher_session_id,
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

        let (subscribe_id, track_alias) = resp_rx.await.unwrap();

        Ok((subscribe_id, track_alias))
    }
    async fn register_pubsup_relation(
        &self,
        publisher_session_id: usize,
        published_subscribe_id: u64,
        subscriber_session_id: usize,
        subscribed_subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = RegisterPubSupRelation {
            publisher_session_id,
            published_subscribe_id,
            subscriber_session_id,
            subscribed_subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("failure"),
        }
    }
    async fn activate_subscriber_subscription(
        &self,
        subscriber_session_id: usize,
        subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = ActivateSubscriberSubscription {
            subscriber_session_id,
            subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("failure"),
        }
    }
    async fn activate_publisher_subscription(
        &self,
        publisher_session_id: usize,
        subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = ActivatePublisherSubscription {
            publisher_session_id,
            subscribe_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("failure"),
        }
    }
    async fn delete_publisher_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = DeletePublisherAnnouncedNamespace {
            track_namespace,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("failure"),
        }
    }
    async fn delete_client(&self, session_id: usize) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();
        let cmd = DeleteClient {
            session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();
        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("failure"),
        }
    }
}

#[cfg(test)]
mod success {
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackNamespaceManager, TrackNamespaceManagerRepository,
    };
    use crate::TrackCommand;
    use tokio::sync::mpsc;
    #[tokio::test]
    async fn set_publisher() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let result = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_publisher_by_namespace() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let _ = track_namespace_manager
            .delete_publisher_by_namespace(track_namespace.clone())
            .await;

        let result = track_namespace_manager
            .get_publisher_session_id(track_namespace.clone())
            .await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn has_track_namespace() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .has_track_namespace(track_namespace.clone())
            .await;

        assert!(result);
    }

    #[tokio::test]
    async fn get_publisher_session_id() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .get_publisher_session_id(track_namespace.clone())
            .await;

        assert_eq!(result, Some(publisher_session_id));
    }

    #[tokio::test]
    async fn set_subscriber_track_not_existed() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_subscriber_track_already_existed() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 1;
        let subscriber_session_id_2 = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
            .await;

        // Register a new subscriber with the same track
        let result = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_2, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_subscriber() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 2;
        let subscriber_session_id_2 = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_2, track_name)
            .await;

        let _ = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id_1)
            .await;

        let result = track_namespace_manager
            .get_subscriber_session_ids_by_track_namespace_and_track_name(
                track_namespace.clone(),
                track_name,
            )
            .await
            .unwrap();
        let expected_result = vec![subscriber_session_id_2];

        assert_eq!(result, expected_result);
    }

    #[tokio::test]
    async fn delete_last_subscriber() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        let _ = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id)
            .await;

        let result = track_namespace_manager
            .has_track_name(track_namespace.clone(), track_name)
            .await;

        assert!(!result);
    }

    #[tokio::test]
    async fn get_subscriber_session_ids_by_track_id() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_ids = vec![2, 3];
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(
                track_namespace.clone(),
                subscriber_session_ids[0],
                track_name,
            )
            .await;
        let _ = track_namespace_manager
            .set_subscriber(
                track_namespace.clone(),
                subscriber_session_ids[1],
                track_name,
            )
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace.clone(), track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespace.clone(),
                track_name,
                subscriber_session_ids[0],
            )
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespace.clone(),
                track_name,
                subscriber_session_ids[1],
            )
            .await;

        let mut result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await
            .unwrap();

        result.sort();

        assert_eq!(result, subscriber_session_ids);
    }

    #[tokio::test]
    async fn delete_client() {
        let track_namespaces = [
            Vec::from(["test1".to_string(), "test1".to_string()]),
            Vec::from(["test2".to_string(), "test2".to_string()]),
        ];
        let publisher_session_ids = [1, 2];
        let mut subscriber_session_ids = vec![2, 3, 4];
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        // Register:
        //   pub 1 <- sub 2, 3, 4
        //   pub 2 <- sub 3, 4
        for i in [0, 1] {
            let _ = track_namespace_manager
                .set_publisher(track_namespaces[i].clone(), publisher_session_ids[i])
                .await;
        }
        let _ = track_namespace_manager
            .set_subscriber(
                track_namespaces[0].clone(),
                subscriber_session_ids[0],
                track_name,
            )
            .await;
        for i in [0, 1] {
            let _ = track_namespace_manager
                .set_subscriber(
                    track_namespaces[i].clone(),
                    subscriber_session_ids[1],
                    track_name,
                )
                .await;
            let _ = track_namespace_manager
                .set_subscriber(
                    track_namespaces[i].clone(),
                    subscriber_session_ids[2],
                    track_name,
                )
                .await;

            let _ = track_namespace_manager
                .set_track_id(track_namespaces[i].clone(), track_name, track_id)
                .await;

            let _ = track_namespace_manager
                .activate_subscriber(
                    track_namespaces[i].clone(),
                    track_name,
                    subscriber_session_ids[1],
                )
                .await;
            let _ = track_namespace_manager
                .activate_subscriber(
                    track_namespaces[i].clone(),
                    track_name,
                    subscriber_session_ids[2],
                )
                .await;
        }
        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespaces[0].clone(),
                track_name,
                subscriber_session_ids[0],
            )
            .await;

        // Delete: pub 2, sub 2
        // Remain: pub 1 <- sub 3, 4
        let _ = track_namespace_manager
            .delete_client(subscriber_session_ids[0])
            .await;

        // Test for subscriber
        // Remain: sub 3, 4
        subscriber_session_ids.remove(0);
        let mut delete_subscriber_result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await
            .unwrap();

        delete_subscriber_result.sort();

        assert_eq!(delete_subscriber_result, subscriber_session_ids);

        // Test for subscriber
        // Remain: pub 1
        let delete_publisher_result_1 = track_namespace_manager
            .get_publisher_session_id(track_namespaces[0].clone())
            .await
            .unwrap();
        let delete_publisher_result_2 = track_namespace_manager
            .get_publisher_session_id(track_namespaces[1].clone())
            .await;

        assert_eq!(delete_publisher_result_1, publisher_session_ids[0]);
        assert!(delete_publisher_result_2.is_none());
    }

    #[tokio::test]
    async fn delete_client_as_last_subscriber() {
        let track_namespaces = [
            Vec::from(["test1".to_string(), "test1".to_string()]),
            Vec::from(["test2".to_string(), "test2".to_string()]),
        ];
        let publisher_session_ids = [1, 2];
        let subscriber_session_id = 2;
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        // Register:
        //   pub 1 <- sub 2
        //   pub 2
        for i in [0, 1] {
            let _ = track_namespace_manager
                .set_publisher(track_namespaces[i].clone(), publisher_session_ids[i])
                .await;
        }

        let _ = track_namespace_manager
            .set_subscriber(
                track_namespaces[0].clone(),
                subscriber_session_id,
                track_name,
            )
            .await;

        let _ = track_namespace_manager
            .set_track_id(track_namespaces[0].clone(), track_name, track_id)
            .await;

        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespaces[0].clone(),
                track_name,
                subscriber_session_id,
            )
            .await;

        // Delete: pub 2, sub 2
        // Remain: pub 1
        let _ = track_namespace_manager
            .delete_client(subscriber_session_id)
            .await;

        let result = track_namespace_manager
            .has_track_name(track_namespaces[0].clone(), track_name)
            .await;

        assert!(!result);
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackNamespaceManager, TrackNamespaceManagerRepository,
    };
    use crate::TrackCommand;
    use tokio::sync::mpsc;
    #[tokio::test]
    async fn set_publisher_already_exist() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_publisher_by_namespace_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .delete_publisher_by_namespace(track_namespace)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_publisher_session_id_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .get_publisher_session_id(track_namespace)
            .await;

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn set_subscriber_already_exist() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        // Register the same subscriber
        let result = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_subscriber_track_namespace_not_found() {
        let track_namespace_1 = Vec::from(["test".to_string(), "test".to_string()]);
        let track_namespace_2 = Vec::from(["unexisted".to_string(), "namespace".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace_1, publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_namespace_manager
            .set_subscriber(track_namespace_2, subscriber_session_id, track_name)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_subscriber_id_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 2;
        let subscriber_session_id_2 = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id_2)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_track_name_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_track_namespace_not_found() {
        let track_namespace_1 = Vec::from(["test".to_string(), "test".to_string()]);
        let track_namespace_2 = Vec::from(["unexisted".to_string(), "namespace".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace_1.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace_1.clone(), subscriber_session_id, track_name)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace_2.clone(), track_name, subscriber_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_subscriber_session_ids_by_track_id_not_found() {
        let track_id = 3;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await;

        assert_eq!(result, None);
    }
}
