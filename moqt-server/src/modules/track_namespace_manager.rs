use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
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
    ) -> Result<()> {
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

        Ok(())
    }

    fn get_subscribers(
        &self,
        publisher_session_id: PublisherSessionId,
        published_subscribe_id: PublishedSubscribeId,
    ) -> Option<&Vec<(SubscriberSessionId, SubscribedSubscribeId)>> {
        let key = (publisher_session_id, published_subscribe_id);
        self.records.get(&key)
    }

    // TODO: Define the behavior if the last subscriber unsubscribes from the track
    // fn delete_relation
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
                // Return an error if it already exists
                if consumers.contains_key(&publisher_session_id) {
                    let msg = "publisher already exists";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }
                consumers.insert(publisher_session_id, Consumer::new(max_subscribe_id));
                resp.send(Ok(())).unwrap();
            }
            SetPublisherAnnouncedNamespace {
                track_namespace,
                publisher_session_id,
                resp,
            } => {
                let consumer = consumers.get_mut(&publisher_session_id).unwrap();
                match consumer.set_namespace(track_namespace) {
                    Ok(_) => {
                        resp.send(Ok(())).unwrap();
                    }
                    Err(err) => {
                        let msg = format!("set_namespace: err: {:?}", err.to_string());
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                    }
                }
            }
            SetupSubscriber {
                max_subscribe_id,
                subscriber_session_id,
                resp,
            } => {
                // Return an error if it already exists
                if producers.contains_key(&subscriber_session_id) {
                    let msg = "subscriber already exists";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }

                producers.insert(subscriber_session_id, Producer::new(max_subscribe_id));
                resp.send(Ok(())).unwrap();
            }
            IsValidSubscriberSubscribeId {
                subscribe_id,
                subscriber_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&subscriber_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let is_valid = producer.is_within_max_subscribe_id(subscribe_id)
                    && producer.is_subscribe_id_unique(subscribe_id);

                resp.send(Ok(is_valid)).unwrap();
            }
            IsValidSubscriberTrackAlias {
                track_alias,
                subscriber_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&subscriber_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let is_valid = producer.is_track_alias_unique(track_alias);
                resp.send(Ok(is_valid)).unwrap();
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
                resp.send(Ok(publisher_session_id)).unwrap();
            }
            GetRequestingSubscriberSessionIdsAndSubscribeIds {
                published_subscribe_id,
                publisher_session_id,
                resp,
            } => {
                if !pubsub_relation
                    .records
                    .contains_key(&(publisher_session_id, published_subscribe_id))
                {
                    let msg = "publisher not found in pubsub relation";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }

                let subscribers =
                    pubsub_relation.get_subscribers(publisher_session_id, published_subscribe_id);

                // Check if it is in the requesting state
                let requesting_subscribers: Option<Vec<(usize, u64)>> = match subscribers {
                    Some(subscribers) => {
                        let mut requesting_subscribers = vec![];

                        for (subscriber_session_id, subscribed_subscribe_id) in subscribers {
                            let producer = producers.get(subscriber_session_id).unwrap();
                            if producer.is_requesting(*subscribed_subscribe_id) {
                                requesting_subscribers
                                    .push((*subscriber_session_id, *subscribed_subscribe_id));
                            }
                        }

                        Some(requesting_subscribers)
                    }
                    None => None,
                };

                resp.send(Ok(requesting_subscribers)).unwrap();
            }
            GetPublisherSubscribeId {
                track_namespace,
                track_name,
                publisher_session_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&publisher_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let result = consumer.get_subscribe_id(track_namespace, track_name);

                resp.send(result).unwrap();
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
                resp.send(Ok(is_existing)).unwrap();
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

                resp.send(result).unwrap();
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
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&subscriber_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

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
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_subscriber_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                        continue;
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
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&publisher_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let (subscribe_id, track_alias) =
                    match consumer.find_unused_subscribe_id_and_track_alias() {
                        Ok(result) => result,
                        Err(err) => {
                            tracing::error!(
                                "find_unused_subscribe_id_and_track_alias: err: {:?}",
                                err.to_string()
                            );
                            resp.send(Err(anyhow!(err))).unwrap();
                            continue;
                        }
                    };
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
                    Ok(_) => resp.send(Ok((subscribe_id, track_alias))).unwrap(),
                    Err(err) => {
                        tracing::error!("set_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
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
                let result = pubsub_relation.add_relation(
                    publisher_session_id,
                    published_subscribe_id,
                    subscriber_session_id,
                    subscribed_subscribe_id,
                );

                match result {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("add_relation: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            ActivateSubscriberSubscription {
                subscriber_session_id,
                subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&subscriber_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match producer.activate_subscription(subscribe_id) {
                    Ok(activation_occured) => {
                        // Return bool as a activation is occurred or not
                        resp.send(Ok(activation_occured)).unwrap()
                    }
                    Err(err) => {
                        tracing::error!("activate_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            ActivatePublisherSubscription {
                publisher_session_id,
                subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&publisher_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match consumer.activate_subscription(subscribe_id) {
                    Ok(activation_occured) => {
                        // Return bool as a activation is occurred or not
                        resp.send(Ok(activation_occured)).unwrap()
                    }
                    Err(err) => {
                        tracing::error!("activate_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            DeletePublisherAnnouncedNamespace {
                track_namespace,
                publisher_session_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&publisher_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                // Return false if the track_namespace does not exist
                if !consumer.has_namespace(track_namespace.clone()) {
                    let msg = "track_namespace not found";
                    tracing::error!(msg);
                    resp.send(Ok(false)).unwrap();
                    continue;
                }

                match consumer.delete_namespace(track_namespace) {
                    Ok(_) => {
                        resp.send(Ok(true)).unwrap();
                    }
                    Err(err) => {
                        tracing::error!("delete_namespace: err: {:?}", err.to_string());
                        resp.send(Err(err)).unwrap();
                    }
                }
            }
            DeleteClient { session_id, resp } => {
                let is_exist_as_consumer = consumers.contains_key(&session_id);
                let is_exist_as_producer = producers.contains_key(&session_id);
                let is_exist_in_pubsub_relation = pubsub_relation
                    .records
                    .iter()
                    .any(|(key, _)| key.0 == session_id);

                // Return an error if the session does not exist
                if !is_exist_as_consumer && !is_exist_as_producer && !is_exist_in_pubsub_relation {
                    let msg = "session not found";
                    tracing::error!(msg);
                    resp.send(Ok(false)).unwrap();
                    continue;
                }

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

                resp.send(Ok(true)).unwrap();
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
        published_subscribe_id: u64,
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
    RegisterPubSupRelation {
        publisher_session_id: usize,
        published_subscribe_id: u64,
        subscriber_session_id: usize,
        subscribed_subscribe_id: u64,
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
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = TrackCommand::SetupPublisher {
            max_subscribe_id,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn set_publisher_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = TrackCommand::SetPublisherAnnouncedNamespace {
            track_namespace,
            publisher_session_id,
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
        subscriber_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = TrackCommand::SetupSubscriber {
            max_subscribe_id,
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn is_valid_subscriber_subscribe_id(
        &self,
        subscribe_id: u64,
        subscriber_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = TrackCommand::IsValidSubscriberSubscribeId {
            subscribe_id,
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_valid) => Ok(is_valid),
            Err(err) => bail!(err),
        }
    }
    async fn is_valid_subscriber_track_alias(
        &self,
        track_alias: u64,
        subscriber_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = TrackCommand::IsValidSubscriberTrackAlias {
            track_alias,
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(is_valid) => Ok(is_valid),
            Err(err) => bail!(err),
        }
    }
    async fn is_track_existing(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = TrackCommand::IsTrackExisting {
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
    async fn get_publisher_subscription_by_full_track_name(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Result<Option<Subscription>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<Subscription>>>();
        let cmd = TrackCommand::GetPublisherSubscription {
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
    async fn get_publisher_session_id(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<Option<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<usize>>>();
        let cmd = TrackCommand::GetPublisherSessionId {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(publisher_session_id) => Ok(publisher_session_id),
            Err(err) => bail!(err),
        }
    }
    async fn get_requesting_subscriber_session_ids_and_subscribe_ids(
        &self,
        published_subscribe_id: u64,
        publisher_session_id: usize,
    ) -> Result<Option<Vec<(usize, u64)>>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<Vec<(usize, u64)>>>>();
        let cmd = TrackCommand::GetRequestingSubscriberSessionIdsAndSubscribeIds {
            published_subscribe_id,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(requesting_subscribers) => Ok(requesting_subscribers),
            Err(err) => bail!(err),
        }
    }
    async fn get_publisher_subscribe_id(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
        publisher_session_id: usize,
    ) -> Result<Option<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<u64>>>();
        let cmd = TrackCommand::GetPublisherSubscribeId {
            track_namespace,
            track_name,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subscribe_id) => Ok(subscribe_id),
            Err(err) => bail!(err),
        }
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
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
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
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
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
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(u64, u64)>>();
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

        let result = resp_rx.await.unwrap();

        match result {
            Ok((subscribe_id, track_alias)) => Ok((subscribe_id, track_alias)),
            Err(err) => bail!(err),
        }
    }
    async fn register_pubsup_relation(
        &self,
        publisher_session_id: usize,
        published_subscribe_id: u64,
        subscriber_session_id: usize,
        subscribed_subscribe_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();
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
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
    async fn activate_subscriber_subscription(
        &self,
        subscriber_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = ActivateSubscriberSubscription {
            subscriber_session_id,
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
    async fn activate_publisher_subscription(
        &self,
        publisher_session_id: usize,
        subscribe_id: u64,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = ActivatePublisherSubscription {
            publisher_session_id,
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
    async fn delete_publisher_announced_namespace(
        &self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();
        let cmd = DeletePublisherAnnouncedNamespace {
            track_namespace,
            publisher_session_id,
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
}

// TrackNamespaceManagerの中身を一新したのですべてのテストを新たに書き直す
#[cfg(test)]
mod success {
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::subscription_models::subscriptions::Subscription;
    use moqt_core::TrackNamespaceManagerRepository;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn setup_publisher() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let result = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_publisher_announced_namespace() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let result = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace, publisher_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn setup_subscriber() {
        let max_subscribe_id = 10;
        let subscriber_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let result = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn is_valid_subscriber_subscribe_id_valid() {
        let max_subscribe_id = 10;
        let subscribe_id = 1;
        let subscriber_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;

        let is_valid = track_namespace_manager
            .is_valid_subscriber_subscribe_id(subscribe_id, subscriber_session_id)
            .await
            .unwrap();

        assert!(is_valid);
    }

    #[tokio::test]
    async fn is_valid_subscriber_subscribe_id_invalid() {
        let max_subscribe_id = 10;
        let subscribe_id = 11;
        let subscriber_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;

        let is_valid = track_namespace_manager
            .is_valid_subscriber_subscribe_id(subscribe_id, subscriber_session_id)
            .await
            .unwrap();

        assert!(!is_valid);
    }

    #[tokio::test]
    async fn is_valid_subscriber_track_alias_valid() {
        let max_subscribe_id = 10;
        let track_alias = 1;
        let subscriber_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;

        let is_valid = track_namespace_manager
            .is_valid_subscriber_track_alias(track_alias, subscriber_session_id)
            .await
            .unwrap();

        assert!(is_valid);
    }

    #[tokio::test]
    async fn is_valid_subscriber_track_alias_invalid() {
        let max_subscribe_id = 10;
        let subscriber_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber_subscription(
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
            )
            .await;

        let is_valid = track_namespace_manager
            .is_valid_subscriber_track_alias(track_alias, subscriber_session_id)
            .await
            .unwrap();

        assert!(!is_valid);
    }

    #[tokio::test]
    async fn is_track_existing_exists() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;

        let _ = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;

        let _ = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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

        let is_existing = track_namespace_manager
            .is_track_existing(track_namespace, track_name)
            .await
            .unwrap();

        assert!(is_existing);
    }

    #[tokio::test]
    async fn is_track_existing_not_exists() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "test_name".to_string();

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let is_existing = track_namespace_manager
            .is_track_existing(track_namespace, track_name)
            .await
            .unwrap();

        assert!(!is_existing);
    }

    #[tokio::test]
    async fn get_publisher_subscription_by_full_track_name() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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

        let subscription = track_namespace_manager
            .get_publisher_subscription_by_full_track_name(
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
    async fn get_publisher_session_id() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;

        let session_id = track_namespace_manager
            .get_publisher_session_id(track_namespace.clone())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(session_id, publisher_session_id);
    }

    #[tokio::test]
    async fn get_requesting_subscriber_session_ids_and_subscribe_ids() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
        let subscriber_session_ids = [2, 3];
        let subscriber_subscribe_ids = [4, 5];
        let subscriber_track_aliases = [6, 7];
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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
            let _ = track_namespace_manager
                .setup_subscriber(max_subscribe_id, subscriber_session_ids[i])
                .await;
            let _ = track_namespace_manager
                .set_subscriber_subscription(
                    subscriber_session_ids[i],
                    subscriber_subscribe_ids[i],
                    subscriber_track_aliases[i],
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
            let _ = track_namespace_manager
                .register_pubsup_relation(
                    publisher_session_id,
                    publisher_subscribe_id,
                    subscriber_session_ids[i],
                    subscriber_subscribe_ids[i],
                )
                .await;
        }

        let list = track_namespace_manager
            .get_requesting_subscriber_session_ids_and_subscribe_ids(
                publisher_subscribe_id,
                publisher_session_id,
            )
            .await
            .unwrap()
            .unwrap();

        let expected_list = vec![
            (subscriber_session_ids[0], subscriber_subscribe_ids[0]),
            (subscriber_session_ids[1], subscriber_subscribe_ids[1]),
        ];

        assert_eq!(list, expected_list,);
    }

    #[tokio::test]
    async fn get_publisher_subscribe_id() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (expected_publisher_subscribe_id, _) = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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

        let publisher_subscribe_id = track_namespace_manager
            .get_publisher_subscribe_id(track_namespace, track_name, publisher_session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(publisher_subscribe_id, expected_publisher_subscribe_id);
    }

    #[tokio::test]
    async fn set_subscriber_subscription() {
        let max_subscribe_id = 10;
        let subscriber_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let result = track_namespace_manager
            .set_subscriber_subscription(
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
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_publisher_subscription() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let result = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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
    }

    #[tokio::test]
    async fn register_pubsup_relation() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let subscriber_subscribe_id = 3;
        let subscriber_track_alias = 4;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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

        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber_subscription(
                subscriber_session_id,
                subscriber_subscribe_id,
                subscriber_track_alias,
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
        let result = track_namespace_manager
            .register_pubsup_relation(
                publisher_session_id,
                publisher_subscribe_id,
                subscriber_session_id,
                subscriber_subscribe_id,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn activate_subscriber_subscription() {
        let max_subscribe_id = 10;
        let subscriber_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber_subscription(
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
            )
            .await;

        let activate_occured = track_namespace_manager
            .activate_subscriber_subscription(subscriber_session_id, subscribe_id)
            .await
            .unwrap();

        assert!(activate_occured);
    }

    #[tokio::test]
    async fn activate_publisher_subscription() {
        let max_subscribe_id = 10;
        let publisher_session_id = 1;
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = track_namespace_manager
            .set_publisher_subscription(
                publisher_session_id,
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

        let activate_occured = track_namespace_manager
            .activate_publisher_subscription(publisher_session_id, publisher_subscribe_id)
            .await
            .unwrap();

        assert!(activate_occured);
    }

    #[tokio::test]
    async fn delete_publisher_announced_namespace() {
        let max_subscribe_id = 10;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .delete_publisher_announced_namespace(track_namespace, publisher_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_client() {
        let max_subscribe_id = 10;
        let track_namespaces = [
            Vec::from(["test1".to_string(), "test1".to_string()]),
            Vec::from(["test2".to_string(), "test2".to_string()]),
        ];
        let publisher_session_ids = [1, 2];
        let mut publisher_subscribe_ids = vec![];
        let mut subscriber_session_ids = vec![2, 3, 4];
        let subscriber_subscribe_ids = [2, 3, 4];
        let subscriber_track_aliases = [2, 3, 4];
        let track_name = "test_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        // Register:
        //   pub 1 <- sub 2, 3, 4
        //   pub 2 <- sub 3, 4
        for i in [0, 1] {
            // for pub 1, 2
            let _ = track_namespace_manager
                .setup_publisher(max_subscribe_id, publisher_session_ids[i])
                .await;
            let _ = track_namespace_manager
                .set_publisher_announced_namespace(
                    track_namespaces[i].clone(),
                    publisher_session_ids[i],
                )
                .await;
            let (publisher_subscribe_id, _) = track_namespace_manager
                .set_publisher_subscription(
                    publisher_session_ids[i],
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
            publisher_subscribe_ids.push(publisher_subscribe_id);
        }

        for j in [0, 1, 2] {
            // for sub 2, 3, 4
            let _ = track_namespace_manager
                .setup_subscriber(max_subscribe_id, subscriber_session_ids[j])
                .await;
        }

        // for sub 2
        let _ = track_namespace_manager
            .set_subscriber_subscription(
                subscriber_session_ids[0],
                subscriber_subscribe_ids[0],
                subscriber_track_aliases[0],
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
                let _ = track_namespace_manager
                    .set_subscriber_subscription(
                        subscriber_session_ids[j],
                        subscriber_subscribe_ids[j],
                        subscriber_track_aliases[j],
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

                let _ = track_namespace_manager
                    .register_pubsup_relation(
                        publisher_session_ids[i],
                        publisher_subscribe_ids[i],
                        subscriber_session_ids[j],
                        subscriber_subscribe_ids[j],
                    )
                    .await;
                // let _ = track_namespace_manager
                //     .activate_subscriber_subscription(
                //         subscriber_session_ids[j],
                //         subscriber_subscribe_ids[j],
                //     )
                //     .await;

                // let _ = track_namespace_manager
                //     .activate_publisher_subscription(
                //         publisher_session_ids[i],
                //         publisher_subscribe_ids[i],
                //     )
                //     .await;
            }
        }

        // for pub 1 and sub 2
        let _ = track_namespace_manager
            .register_pubsup_relation(
                publisher_session_ids[0],
                publisher_subscribe_ids[0],
                subscriber_session_ids[0],
                subscriber_subscribe_ids[0],
            )
            .await;
        // let _ = track_namespace_manager
        //     .activate_subscriber_subscription(
        //         subscriber_session_ids[0],
        //         subscriber_subscribe_ids[0],
        //     )
        //     .await;

        // let _ = track_namespace_manager
        //     .activate_publisher_subscription(publisher_session_ids[0], publisher_subscribe_ids[0])
        //     .await;

        // Delete: pub 2, sub 2
        // Remain: pub 1 <- sub 3, 4
        let _ = track_namespace_manager
            .delete_client(subscriber_session_ids[0])
            .await;

        // Test for subscriber
        // Remain: sub 3, 4
        subscriber_session_ids.remove(0);

        // TODO: Replace not requesting version
        let mut delete_subscriber_result = track_namespace_manager
            .get_requesting_subscriber_session_ids_and_subscribe_ids(
                publisher_subscribe_ids[0],
                publisher_session_ids[0],
            )
            .await
            .unwrap()
            .unwrap();

        delete_subscriber_result.sort();

        let expected_list = vec![
            (subscriber_session_ids[0], subscriber_subscribe_ids[1]),
            (subscriber_session_ids[1], subscriber_subscribe_ids[2]),
        ];

        assert_eq!(delete_subscriber_result, expected_list);

        // Test for publisher
        // Remain: pub 1
        let delete_publisher_result_1 = track_namespace_manager
            .get_publisher_session_id(track_namespaces[0].clone())
            .await
            .unwrap()
            .unwrap();
        let delete_publisher_result_2 = track_namespace_manager
            .get_publisher_session_id(track_namespaces[1].clone())
            .await
            .unwrap();

        assert_eq!(delete_publisher_result_1, publisher_session_ids[0]);
        assert!(delete_publisher_result_2.is_none());
    }
}

// #[cfg(test)]
// mod failure {
//     use crate::modules::track_namespace_manager::{
//         track_namespace_manager, TrackNamespaceManager, TrackNamespaceManagerRepository,
//     };
//     use crate::TrackCommand;
//     use tokio::sync::mpsc;

//     #[tokio::test]
//     async fn setup_publisher_already_exist() {
//         let max_subscribe_id = 10;
//         let publisher_session_id = 1;

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .setup_publisher(max_subscribe_id, publisher_session_id)
//             .await;

//         // Register the same publisher
//         let result = track_namespace_manager
//             .setup_publisher(max_subscribe_id, publisher_session_id)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn set_publisher_announced_namespace_already_exist() {
//         let max_subscribe_id = 10;
//         let publisher_session_id = 1;
//         let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .setup_publisher(max_subscribe_id, publisher_session_id)
//             .await;
//         let _ = track_namespace_manager
//             .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
//             .await;

//         // Register the same track namespace
//         let result = track_namespace_manager
//             .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn setup_subscriber_already_exist() {
//         let max_subscribe_id = 10;
//         let subscriber_session_id = 1;

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .setup_subscriber(max_subscribe_id, subscriber_session_id)
//             .await;

//         // Register the same subscriber
//         let result = track_namespace_manager
//             .setup_subscriber(max_subscribe_id, subscriber_session_id)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn is_valid_subscriber_subscribe_id_subscriber_not_found() {
//         let max_subscribe_id = 10;
//         let subscriber_session_id = 1;
//         let subscriber_subscribe_id = 0;
//         let invalid_subscriber_session_id = 2;

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .setup_subscriber(max_subscribe_id, subscriber_session_id)
//             .await;

//         let result = track_namespace_manager
//             .is_valid_subscriber_subscribe_id(
//                 subscriber_subscribe_id,
//                 invalid_subscriber_session_id,
//             )
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn is_valid_subscriber_track_alias_subscriber_not_found() {
//         let max_subscribe_id = 10;
//         let subscriber_session_id = 1;
//         let subscriber_track_alias = 0;
//         let invalid_subscriber_session_id = 2;

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .setup_subscriber(max_subscribe_id, subscriber_session_id)
//             .await;

//         let result = track_namespace_manager
//             .is_valid_subscriber_track_alias(subscriber_track_alias, invalid_subscriber_session_id)
//             .await;

//         assert!(result.is_err());
//     }
// }

//     #[tokio::test]
//     async fn delete_publisher_by_namespace_not_found() {
//         let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

//         let result = track_namespace_manager
//             .delete_publisher_by_namespace(track_namespace)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn get_publisher_session_id_not_found() {
//         let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

//         let result = track_namespace_manager
//             .get_publisher_session_id(track_namespace)
//             .await;

//         assert_eq!(result, None);
//     }

//     #[tokio::test]
//     async fn set_subscriber_already_exist() {
//         let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
//         let publisher_session_id = 1;
//         let subscriber_session_id = 2;
//         let track_name = "test_name";

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .set_publisher(track_namespace.clone(), publisher_session_id)
//             .await;
//         let _ = track_namespace_manager
//             .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
//             .await;

//         // Register the same subscriber
//         let result = track_namespace_manager
//             .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn set_subscriber_track_namespace_not_found() {
//         let track_namespace_1 = Vec::from(["test".to_string(), "test".to_string()]);
//         let track_namespace_2 = Vec::from(["unexisted".to_string(), "namespace".to_string()]);
//         let publisher_session_id = 1;
//         let subscriber_session_id = 2;
//         let track_name = "test_name";

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .set_publisher(track_namespace_1, publisher_session_id)
//             .await;

//         // Register a new subscriber with a new track
//         let result = track_namespace_manager
//             .set_subscriber(track_namespace_2, subscriber_session_id, track_name)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn delete_subscriber_subscriber_id_not_found() {
//         let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
//         let publisher_session_id = 1;
//         let subscriber_session_id_1 = 2;
//         let subscriber_session_id_2 = 3;
//         let track_name = "test_name";

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .set_publisher(track_namespace.clone(), publisher_session_id)
//             .await;
//         let _ = track_namespace_manager
//             .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
//             .await;

//         let result = track_namespace_manager
//             .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id_2)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn delete_subscriber_track_name_not_found() {
//         let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
//         let publisher_session_id = 1;
//         let subscriber_session_id = 2;
//         let track_name = "test_name";

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .set_publisher(track_namespace.clone(), publisher_session_id)
//             .await;

//         let result = track_namespace_manager
//             .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn delete_subscriber_track_namespace_not_found() {
//         let track_namespace_1 = Vec::from(["test".to_string(), "test".to_string()]);
//         let track_namespace_2 = Vec::from(["unexisted".to_string(), "namespace".to_string()]);
//         let publisher_session_id = 1;
//         let subscriber_session_id = 2;
//         let track_name = "test_name";

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
//         let _ = track_namespace_manager
//             .set_publisher(track_namespace_1.clone(), publisher_session_id)
//             .await;
//         let _ = track_namespace_manager
//             .set_subscriber(track_namespace_1.clone(), subscriber_session_id, track_name)
//             .await;

//         let result = track_namespace_manager
//             .delete_subscriber(track_namespace_2.clone(), track_name, subscriber_session_id)
//             .await;

//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn get_subscriber_session_ids_by_track_id_not_found() {
//         let track_id = 3;

//         // Start track management thread
//         let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
//         tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

//         let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

//         let result = track_namespace_manager
//             .get_subscriber_session_ids_by_track_id(track_id)
//             .await;

//         assert_eq!(result, None);
//     }
// }
