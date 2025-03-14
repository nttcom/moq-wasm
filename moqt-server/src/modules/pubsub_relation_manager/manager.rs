use anyhow::anyhow;
use std::{collections::HashMap, vec};
use tokio::sync::mpsc;

use moqt_core::models::subscriptions::nodes::{
    consumers::Consumer, producers::Producer, registry::SubscriptionNodeRegistry,
};

use crate::modules::pubsub_relation_manager::{
    commands::{PubSubRelationCommand, PubSubRelationCommand::*},
    relation::PubSubRelation,
};

type DownstreamSessionId = usize;
type UpstreamSessionId = usize;
pub(crate) type Consumers = HashMap<UpstreamSessionId, Consumer>;
pub(crate) type Producers = HashMap<DownstreamSessionId, Producer>;

// [Original Publisher: (Producer) ] -> [Relay: (Consumer) - <PubSubRelation> - (Producer) ] -> [End Subscriber: (Consumer)]

fn is_namespace_prefix_match(
    track_namespace: Vec<String>,
    track_namespace_prefix: Vec<String>,
) -> bool {
    if track_namespace.len() < track_namespace_prefix.len() {
        return false;
    }

    for (i, prefix) in track_namespace_prefix.iter().enumerate() {
        if track_namespace[i] != *prefix {
            return false;
        }
    }

    true
}

pub(crate) async fn pubsub_relation_manager(rx: &mut mpsc::Receiver<PubSubRelationCommand>) {
    // Producers: HashMap<DownstreamSessionId, Producer>
    // {
    //   "${downstream_session_id}" : {
    //      max_subscriber_id: u64,
    //      announcing_namespaces: Vec<TrackNamespace>,
    //      subscribed_namespace_prefixes: Vec<TrackNamespace>,
    //      subscriptions: HashMap<SubscribeId, Subscription>,
    //   }
    // }
    // Consumers: HashMap<UpstreamSessionId, Consumer>
    // {
    //   "${upstream_session_id}" : {
    //      max_subscriber_id: u64,
    //      announced_namespaces: Vec<TrackNamespace>,
    //      subscribing_namespace_prefixes: Vec<TrackNamespace>,
    //      subscriptions: HashMap<SubscribeId, Subscription>,
    //      latest_subscribe_id: u64,
    //   }
    // }
    // PubSubRelation: HashMap<(UpstreamSessionId, UpstreamSubscribeId), Vec<(DownstreamSessionId, DownstreamSubscribeId)>>
    // {
    //     "records": {
    //       (PublisherSessionId, PublisherSubscribeId): Vec<(SubscriberSessionId, SubscriberSubscribeId)>,
    //     }
    //  }
    tracing::trace!("pubsub_relation_manager start");

    let mut consumers: Consumers = HashMap::new();
    let mut producers: Producers = HashMap::new();
    let mut pubsub_relation = PubSubRelation::new();

    while let Some(cmd) = rx.recv().await {
        tracing::trace!("command received: {:#?}", cmd);
        match cmd {
            SetupPublisher {
                max_subscribe_id,
                upstream_session_id,
                resp,
            } => {
                // Return an error if it already exists
                if consumers.contains_key(&upstream_session_id) {
                    let msg = "publisher already exists";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }
                consumers.insert(upstream_session_id, Consumer::new(max_subscribe_id));
                resp.send(Ok(())).unwrap();
            }
            SetUpstreamAnnouncedNamespace {
                track_namespace,
                upstream_session_id,
                resp,
            } => {
                let consumer = match consumers.get_mut(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
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
            SetDownstreamAnnouncedNamespace {
                track_namespace,
                downstream_session_id,
                resp,
            } => {
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                match producer.set_namespace(track_namespace) {
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
            SetDownstreamSubscribedNamespacePrefix {
                track_namespace_prefix,
                downstream_session_id,
                resp,
            } => {
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match producer.set_namespace_prefix(track_namespace_prefix) {
                    Ok(_) => {
                        resp.send(Ok(())).unwrap();
                    }
                    Err(err) => {
                        let msg = format!("set_namespace_prefix: err: {:?}", err.to_string());
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                    }
                }
            }
            SetupSubscriber {
                max_subscribe_id,
                downstream_session_id,
                resp,
            } => {
                // Return an error if it already exists
                if producers.contains_key(&downstream_session_id) {
                    let msg = "subscriber already exists";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }

                producers.insert(downstream_session_id, Producer::new(max_subscribe_id));
                resp.send(Ok(())).unwrap();
            }
            IsDownstreamSubscribeIdUnique {
                subscribe_id,
                downstream_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let is_valid = producer.is_subscribe_id_unique(subscribe_id);
                resp.send(Ok(is_valid)).unwrap();
            }
            IsDownstreamSubscribeIdLessThanMax {
                subscribe_id,
                downstream_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let is_valid = producer.is_subscribe_id_less_than_max(subscribe_id);
                resp.send(Ok(is_valid)).unwrap();
            }
            IsDownstreamTrackAliasUnique {
                track_alias,
                downstream_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let is_unique = producer.is_track_alias_unique(track_alias);
                resp.send(Ok(is_unique)).unwrap();
            }
            GetUpstreamSessionId {
                track_namespace,
                resp,
            } => {
                // Find the publisher that has the track namespace from all consumers
                let upstream_session_id = consumers
                    .iter()
                    .find(|(_, consumer)| consumer.has_namespace(track_namespace.clone()))
                    .map(|(session_id, _)| *session_id);
                resp.send(Ok(upstream_session_id)).unwrap();
            }
            GetRequestingDownstreamSessionIdsAndSubscribeIds {
                upstream_subscribe_id,
                upstream_session_id,
                resp,
            } => {
                if !pubsub_relation
                    .records
                    .contains_key(&(upstream_session_id, upstream_subscribe_id))
                {
                    let msg = "publisher not found in pubsub relation";
                    tracing::error!(msg);
                    resp.send(Err(anyhow!(msg))).unwrap();
                    continue;
                }

                let subscribers =
                    pubsub_relation.get_subscribers(upstream_session_id, upstream_subscribe_id);

                // Check if it is in the requesting state
                let requesting_subscribers: Option<Vec<(usize, u64)>> = match subscribers {
                    Some(subscribers) => {
                        let mut requesting_subscribers = vec![];

                        for (downstream_session_id, downstream_subscribe_id) in subscribers {
                            let producer = producers.get(downstream_session_id).unwrap();
                            if producer.is_requesting(*downstream_subscribe_id) {
                                requesting_subscribers
                                    .push((*downstream_session_id, *downstream_subscribe_id));
                            }
                        }

                        Some(requesting_subscribers)
                    }
                    None => None,
                };

                resp.send(Ok(requesting_subscribers)).unwrap();
            }
            GetUpstreamSubscribeId {
                track_namespace,
                track_name,
                upstream_session_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
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
            GetUpstreamSubscribeIdByTrackAlias {
                upstream_session_id,
                upstream_track_alias,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let result = consumer.get_subscribe_id_by_track_alias(upstream_track_alias);

                resp.send(result).unwrap();
            }
            GetDownstreamTrackAlias {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let track_alias = producer.get_track_alias(downstream_subscribe_id).unwrap();
                resp.send(Ok(track_alias)).unwrap();
            }
            IsUpstreamSubscribed {
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
            SetDownstreamSubscription {
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
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
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
                ) {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_downstream_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                        continue;
                    }
                }
            }
            SetUpstreamSubscription {
                upstream_session_id,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let (subscribe_id, track_alias) =
                    match consumer.create_latest_subscribe_id_and_track_alias() {
                        Ok(result) => result,
                        Err(err) => {
                            tracing::error!(
                                "create_latest_subscribe_id_and_track_alias: err: {:?}",
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
                ) {
                    Ok(_) => resp.send(Ok((subscribe_id, track_alias))).unwrap(),
                    Err(err) => {
                        tracing::error!("set_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                        continue;
                    }
                };
            }
            SetPubSubRelation {
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                let result = pubsub_relation.add_relation(
                    upstream_session_id,
                    upstream_subscribe_id,
                    downstream_session_id,
                    downstream_subscribe_id,
                );

                match result {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("add_relation: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            ActivateDownstreamSubscription {
                downstream_session_id,
                subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
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
            ActivateUpstreamSubscription {
                upstream_session_id,
                subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&upstream_session_id) {
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
            GetUpstreamNamespacesMatchesPrefix {
                track_namespace_prefix,
                resp,
            } => {
                let mut namespaces = vec![];
                for consumer in consumers.values() {
                    for consumer_namespace in consumer.get_namespaces().unwrap() {
                        if is_namespace_prefix_match(
                            consumer_namespace.clone(),
                            track_namespace_prefix.clone(),
                        ) {
                            namespaces.push(consumer_namespace.clone());
                        }
                    }
                }
                resp.send(Ok(namespaces)).unwrap();
            }
            IsNamespaceAlreadyAnnounced {
                track_namespace,
                downstream_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let is_announced = producer.has_namespace(track_namespace);
                resp.send(Ok(is_announced)).unwrap();
            }
            GetDownstreamSessionIdsByUpstreamNamespace {
                track_namespace,
                resp,
            } => {
                let mut downstream_session_ids = vec![];
                for (downstream_session_id, producer) in &producers {
                    let downstream_subscribe_prefix = producer.get_namespace_prefixes().unwrap();
                    for prefix in downstream_subscribe_prefix {
                        if is_namespace_prefix_match(track_namespace.clone(), prefix.clone()) {
                            downstream_session_ids.push(*downstream_session_id);
                        }
                    }
                }

                resp.send(Ok(downstream_session_ids)).unwrap();
            }
            DeleteUpstreamAnnouncedNamespace {
                track_namespace,
                upstream_session_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&upstream_session_id) {
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
                        subscribers.retain(|(downstream_session_id, _)| {
                            *downstream_session_id != session_id
                        });
                    });

                resp.send(Ok(true)).unwrap();
            }
            DeletePubSubRelation {
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                let result = pubsub_relation.delete_relation(
                    upstream_session_id,
                    upstream_subscribe_id,
                    downstream_session_id,
                    downstream_subscribe_id,
                );

                match result {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("delete_relation: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            DeleteUpstreamSubscription {
                upstream_session_id,
                upstream_subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match consumer.delete_subscription(upstream_subscribe_id) {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("delete_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            DeleteDownstreamSubscription {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match producer.delete_subscription(downstream_subscribe_id) {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("delete_subscription: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            SetDownstreamForwardingPreference {
                downstream_session_id,
                downstream_subscribe_id,
                forwarding_preference,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                match producer
                    .set_forwarding_preference(downstream_subscribe_id, forwarding_preference)
                {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_forwarding_preference: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            SetUpstreamForwardingPreference {
                upstream_session_id,
                upstream_subscribe_id,
                forwarding_preference,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match consumer
                    .set_forwarding_preference(upstream_subscribe_id, forwarding_preference)
                {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_forwarding_preference: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            GetUpstreamForwardingPreference {
                upstream_session_id,
                upstream_subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let forwarding_preference = consumer
                    .get_forwarding_preference(upstream_subscribe_id)
                    .unwrap();
                resp.send(Ok(forwarding_preference)).unwrap();
            }
            GetUpstreamFilterType {
                upstream_session_id,
                upstream_subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let filter_type = consumer.get_filter_type(upstream_subscribe_id).unwrap();
                resp.send(Ok(filter_type)).unwrap();
            }
            GetDownstreamFilterType {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let filter_type = producer.get_filter_type(downstream_subscribe_id).unwrap();
                resp.send(Ok(filter_type)).unwrap();
            }
            GetUpstreamRequestedObjectRange {
                upstream_session_id,
                upstream_subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let range = consumer
                    .get_requested_object_range(upstream_subscribe_id)
                    .unwrap();
                resp.send(Ok(range)).unwrap();
            }
            GetDownstreamRequestedObjectRange {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let range = producer
                    .get_requested_object_range(downstream_subscribe_id)
                    .unwrap();
                resp.send(Ok(range)).unwrap();
            }
            SetDownstreamActualObjectStart {
                downstream_session_id,
                downstream_subscribe_id,
                actual_object_start,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                match producer.set_actual_object_start(downstream_subscribe_id, actual_object_start)
                {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_actual_object_start: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            GetDownstreamActualObjectStart {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let actual_object_start = producer
                    .get_actual_object_start(downstream_subscribe_id)
                    .unwrap();
                resp.send(Ok(actual_object_start)).unwrap();
            }
            GetRelatedSubscribers {
                upstream_session_id,
                upstream_subscribe_id,
                resp,
            } => {
                let subscribers =
                    pubsub_relation.get_subscribers(upstream_session_id, upstream_subscribe_id);

                let subscribers = match subscribers {
                    Some(subscribers) => subscribers.clone(),
                    None => vec![],
                };

                resp.send(Ok(subscribers)).unwrap();
            }
            SetUpstreamStreamId {
                upstream_session_id,
                upstream_subscribe_id,
                group_id,
                subgroup_id,
                stream_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get_mut(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match consumer.set_stream_id(
                    upstream_subscribe_id,
                    group_id,
                    subgroup_id,
                    stream_id,
                ) {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_stream_id: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            GetUpstreamSubscribeIdsForClient {
                upstream_session_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                let subscribe_ids = consumer.get_all_subscribe_ids().unwrap();
                resp.send(Ok(subscribe_ids)).unwrap();
            }
            GetUpstreamGroupIdsForSubscription {
                upstream_session_id,
                upstream_subscribe_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                let group_ids = consumer
                    .get_group_ids_for_subscription(upstream_subscribe_id)
                    .unwrap();
                resp.send(Ok(group_ids)).unwrap();
            }
            GetUpstreamSubgroupIdsForGroup {
                upstream_session_id,
                upstream_subscribe_id,
                group_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let subgroup_ids = consumer
                    .get_subgroup_ids_for_group(upstream_subscribe_id, group_id)
                    .unwrap();
                resp.send(Ok(subgroup_ids)).unwrap();
            }
            GetUpstreamStreamIdForSubgroup {
                upstream_session_id,
                upstream_subscribe_id,
                group_id,
                subgroup_id,
                resp,
            } => {
                // Return an error if the publisher does not exist
                let consumer = match consumers.get(&upstream_session_id) {
                    Some(consumer) => consumer,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let stream_id = consumer
                    .get_stream_id_for_subgroup(upstream_subscribe_id, group_id, subgroup_id)
                    .unwrap();
                resp.send(Ok(stream_id)).unwrap();
            }
            SetDownstreamStreamId {
                downstream_session_id,
                downstream_subscribe_id,
                group_id,
                subgroup_id,
                stream_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get_mut(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                match producer.set_stream_id(
                    downstream_subscribe_id,
                    group_id,
                    subgroup_id,
                    stream_id,
                ) {
                    Ok(_) => resp.send(Ok(())).unwrap(),
                    Err(err) => {
                        tracing::error!("set_stream_id: err: {:?}", err.to_string());
                        resp.send(Err(anyhow!(err))).unwrap();
                    }
                }
            }
            GetDownstreamSubscribeIdsForClient {
                downstream_session_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                let subscribe_ids = producer.get_all_subscribe_ids().unwrap();
                resp.send(Ok(subscribe_ids)).unwrap();
            }
            GetDownstreamGroupIdsForSubscription {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };
                let group_ids = producer
                    .get_group_ids_for_subscription(downstream_subscribe_id)
                    .unwrap();
                resp.send(Ok(group_ids)).unwrap();
            }
            GetDownstreamSubgroupIdsForGroup {
                downstream_session_id,
                downstream_subscribe_id,
                group_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let stream_ids = producer
                    .get_subgroup_ids_for_group(downstream_subscribe_id, group_id)
                    .unwrap();
                resp.send(Ok(stream_ids)).unwrap();
            }
            GetDownstreamStreamIdForSubgroup {
                downstream_session_id,
                downstream_subscribe_id,
                group_id,
                subgroup_id,
                resp,
            } => {
                // Return an error if the subscriber does not exist
                let producer = match producers.get(&downstream_session_id) {
                    Some(producer) => producer,
                    None => {
                        let msg = "subscriber not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                let stream_id = producer
                    .get_stream_id_for_subgroup(downstream_subscribe_id, group_id, subgroup_id)
                    .unwrap();
                resp.send(Ok(stream_id)).unwrap();
            }
            GetRelatedPublisher {
                downstream_session_id,
                downstream_subscribe_id,
                resp,
            } => {
                let publisher =
                    pubsub_relation.get_publisher(downstream_session_id, downstream_subscribe_id);

                let publisher = match publisher {
                    Some(publisher) => publisher,
                    None => {
                        let msg = "publisher not found";
                        tracing::error!(msg);
                        resp.send(Err(anyhow!(msg))).unwrap();
                        continue;
                    }
                };

                resp.send(Ok(publisher)).unwrap();
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
