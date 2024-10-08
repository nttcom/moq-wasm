use crate::modules::pubsub_relation_manager::{
    commands::{PubSubRelationCommand, PubSubRelationCommand::*},
    relation::PubSubRelation,
};
use anyhow::anyhow;
use moqt_core::subscription_models::nodes::{
    consumer_node::Consumer, node_registory::SubscriptionNodeRegistory, producer_node::Producer,
};
use std::collections::HashMap;
use tokio::sync::mpsc;

type SubscriberSessionId = usize;
type PublisherSessionId = usize;

pub(crate) type Consumers = HashMap<PublisherSessionId, Consumer>;
pub(crate) type Producers = HashMap<SubscriberSessionId, Producer>;

// [Original Publisher: (Producer) ] -> [Relay: (Consumer) - <PubSubRelation> - (Producer) ] -> [End Subscriber: (Consumer)]

// Called as a separate thread
pub(crate) async fn pubsub_relation_manager(rx: &mut mpsc::Receiver<PubSubRelationCommand>) {
    tracing::trace!("pubsub_relation_manager start");

    let mut consumers: Consumers = HashMap::new();
    let mut producers: Producers = HashMap::new();
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
                publisher_subscribe_id,
                publisher_session_id,
                resp,
            } => {
                if !pubsub_relation
                    .records
                    .contains_key(&(publisher_session_id, publisher_subscribe_id))
                {
                    let msg = "publisher not found in pubsub relation";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }

                let subscribers =
                    pubsub_relation.get_subscribers(publisher_session_id, publisher_subscribe_id);

                // Check if it is in the requesting state
                let requesting_subscribers: Option<Vec<(usize, u64)>> = match subscribers {
                    Some(subscribers) => {
                        let mut requesting_subscribers = vec![];

                        for (subscriber_session_id, subscriber_subscribe_id) in subscribers {
                            let producer = producers.get(subscriber_session_id).unwrap();
                            if producer.is_requesting(*subscriber_subscribe_id) {
                                requesting_subscribers
                                    .push((*subscriber_session_id, *subscriber_subscribe_id));
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
            RegisterPubSubRelation {
                publisher_session_id,
                publisher_subscribe_id,
                subscriber_session_id,
                subscriber_subscribe_id,
                resp,
            } => {
                let result = pubsub_relation.add_relation(
                    publisher_session_id,
                    publisher_subscribe_id,
                    subscriber_session_id,
                    subscriber_subscribe_id,
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
            #[cfg(test)]
            GetNodeAndRelationClone { resp } => {
                let consumer = consumers.clone();
                let producer = producers.clone();
                let relation = pubsub_relation.clone();

                resp.send(Ok((consumer, producer, relation))).unwrap();
            }
        }
    }

    tracing::trace!("pubsub_relation_manager end");
}
