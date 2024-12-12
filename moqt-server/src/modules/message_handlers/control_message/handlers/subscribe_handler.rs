use crate::{modules::moqt_client::MOQTClient, SenderToOpenSubscription};
use anyhow::{bail, Result};
use moqt_core::{
    constants::StreamDirection,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::{
            subscribe::Subscribe,
            subscribe_error::{SubscribeError, SubscribeErrorCode},
            subscribe_ok::SubscribeOk,
        },
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    SendStreamDispatcherRepository,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::modules::object_cache_storage::{CacheHeader, ObjectCacheStorageWrapper};

pub(crate) async fn subscribe_handler(
    subscribe_message: Subscribe,
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_downstream_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
) -> Result<Option<SubscribeError>> {
    tracing::trace!("subscribe_handler start.");

    tracing::debug!("subscribe_message: {:#?}", subscribe_message);

    // TODO: validate Unauthorized

    if !pubsub_relation_manager_repository
        .is_valid_downstream_subscribe_id(subscribe_message.subscribe_id(), client.id())
        .await?
    {
        // TODO: return TerminationErrorCode
        bail!("TooManySubscribers");
    }
    if !pubsub_relation_manager_repository
        .is_valid_downstream_track_alias(subscribe_message.track_alias(), client.id())
        .await?
    {
        // TODO: create accurate track alias
        let reason_phrase = "Invalid Track Alias".to_string();
        let subscribe_error = SubscribeError::new(
            subscribe_message.subscribe_id(),
            SubscribeErrorCode::RetryTrackAlias,
            reason_phrase,
            100, // track alias
        );
        return Ok(Some(subscribe_error));

        // TODO: return TerminationErrorCode::DuplicateTrackAlias
    }

    // TODO: validate Invalid Range

    // If the track exists, return ther track as it is
    if pubsub_relation_manager_repository
        .is_track_existing(
            subscribe_message.track_namespace().to_vec(),
            subscribe_message.track_name().to_string(),
        )
        .await
        .unwrap()
    {
        // Generate message -> Set subscription -> Send message
        let subscribe_ok_message = match generate_subscribe_ok_message(
            pubsub_relation_manager_repository,
            object_cache_storage,
            &subscribe_message,
        )
        .await
        {
            Ok(message) => {
                match set_downstream_subscription(
                    pubsub_relation_manager_repository,
                    &subscribe_message,
                    client,
                )
                .await
                {
                    Ok(_) => {
                        tracing::info!(
                            "subscribed track_namespace: {:?}",
                            subscribe_message.track_namespace(),
                        );
                        tracing::info!(
                            "subscribed track_name: {:?}",
                            subscribe_message.track_name()
                        );
                        tracing::trace!("subscribe_handler complete.");
                    }
                    Err(e) => {
                        let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                        let subscribe_error = SubscribeError::new(
                            subscribe_message.subscribe_id(),
                            SubscribeErrorCode::InternalError,
                            reason_phrase,
                            subscribe_message.track_alias(),
                        );
                        return Ok(Some(subscribe_error));
                    }
                }
                message
            }
            Err(e) => {
                let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                let subscribe_error = SubscribeError::new(
                    subscribe_message.subscribe_id(),
                    SubscribeErrorCode::InternalError,
                    reason_phrase,
                    subscribe_message.track_alias(),
                );
                return Ok(Some(subscribe_error));
            }
        };

        // Send SUBSCRIBE_OK message if generate massage and set subscription is successfully done
        let subscribe_ok_payload: Box<dyn MOQTPayload> = Box::new(subscribe_ok_message.clone());

        // TODO: Unify the method to send a message to the opposite client itself
        send_stream_dispatcher_repository
            .transfer_message_to_send_stream_thread(
                client.id(),
                subscribe_ok_payload,
                StreamDirection::Bi,
            )
            .await?;

        if subscribe_ok_message.content_exists() {
            open_new_subscription(
                pubsub_relation_manager_repository,
                object_cache_storage,
                open_downstream_subscription_txes,
                client,
                subscribe_message,
            )
            .await?;
        }

        return Ok(None);
    }

    // Since only the track_namespace is recorded in ANNOUNCE, use track_namespace to determine the publisher
    // TODO: multiple publishers for the same track_namespace
    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(subscribe_message.track_namespace().clone())
        .await
        .unwrap();
    match upstream_session_id {
        Some(session_id) => {
            let (upstream_subscribe_id, upstream_track_alias) =
                match set_downstream_and_upstream_subscription(
                    pubsub_relation_manager_repository,
                    &subscribe_message,
                    client,
                    session_id,
                )
                .await
                {
                    Ok((upstream_subscribe_id, upstream_track_alias)) => {
                        (upstream_subscribe_id, upstream_track_alias)
                    }
                    Err(e) => {
                        let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                        let subscribe_error = SubscribeError::new(
                            subscribe_message.subscribe_id(),
                            SubscribeErrorCode::InternalError,
                            reason_phrase,
                            subscribe_message.track_alias(),
                        );
                        return Ok(Some(subscribe_error));
                    }
                };

            // Replace the subscribe_id and track_alias in the SUBSCRIBE message to request to the upstream publisher
            // TODO: auth parameter
            let message_payload = Subscribe::new(
                upstream_subscribe_id,
                upstream_track_alias,
                subscribe_message.track_namespace().clone(),
                subscribe_message.track_name().to_string(),
                subscribe_message.subscriber_priority(),
                subscribe_message.group_order(),
                subscribe_message.filter_type(),
                subscribe_message.start_group(),
                subscribe_message.start_object(),
                subscribe_message.end_group(),
                subscribe_message.end_object(),
                subscribe_message.subscribe_parameters().clone(),
            )
            .unwrap();

            let relaying_subscribe_message: Box<dyn MOQTPayload> =
                Box::new(message_payload.clone());

            // Notify to the publisher about the SUBSCRIBE message
            // TODO: Wait for the SUBSCRIBE_OK message to be returned on a transaction
            // TODO: validate Timeout
            match send_stream_dispatcher_repository
                .transfer_message_to_send_stream_thread(
                    session_id,
                    relaying_subscribe_message,
                    StreamDirection::Bi,
                )
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "subscribed track_namespace: {:?}",
                        message_payload.track_namespace(),
                    );
                    tracing::info!("subscribed track_name: {:?}", message_payload.track_name());
                    tracing::trace!("subscribe_handler complete.");
                }
                Err(e) => {
                    let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                    let subscribe_error = SubscribeError::new(
                        subscribe_message.subscribe_id(),
                        SubscribeErrorCode::InternalError,
                        reason_phrase,
                        subscribe_message.track_alias(),
                    );
                    return Ok(Some(subscribe_error));
                }
            }

            tracing::debug!(
                "message: {:#?} is sent to relay handler for client {:?}",
                message_payload.clone(),
                session_id
            );

            Ok(None)
        }

        // TODO: Check if “publisher not found” should turn into closing session
        None => {
            let reason_phrase = "publisher not found".to_string();
            let subscribe_error = SubscribeError::new(
                subscribe_message.subscribe_id(),
                SubscribeErrorCode::TrackDoesNotExist,
                reason_phrase,
                subscribe_message.track_alias(),
            );

            Ok(Some(subscribe_error))
        }
    }
}

async fn open_new_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_downstream_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
    client: &MOQTClient,
    subscribe_message: Subscribe,
) -> Result<()> {
    let downstream_session_id = client.id();
    let downstream_subscribe_id = subscribe_message.subscribe_id();

    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(subscribe_message.track_namespace().clone())
        .await?
        .unwrap();

    let upstream_subscribe_id = pubsub_relation_manager_repository
        .get_upstream_subscribe_id(
            subscribe_message.track_namespace().clone(),
            subscribe_message.track_name().to_string(),
            upstream_session_id,
        )
        .await?
        .unwrap();

    let stream_header_type = match object_cache_storage
        .get_header(upstream_session_id, upstream_subscribe_id)
        .await
    {
        Ok(CacheHeader::Datagram) => DataStreamType::ObjectDatagram,
        Ok(CacheHeader::Track(_)) => DataStreamType::StreamHeaderTrack,
        Ok(CacheHeader::Subgroup(_)) => DataStreamType::StreamHeaderSubgroup,
        Err(_) => bail!("CacheHeader not found"),
    };

    let open_downstream_subscription_tx = open_downstream_subscription_txes
        .lock()
        .await
        .get(&downstream_session_id)
        .unwrap()
        .clone();

    let _ = open_downstream_subscription_tx
        .send((downstream_subscribe_id, stream_header_type.clone()))
        .await;

    Ok(())
}

async fn generate_subscribe_ok_message(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    subscribe_message: &Subscribe,
) -> Result<SubscribeOk> {
    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(subscribe_message.track_namespace().clone())
        .await?
        .unwrap();

    let upstream_subscribe_id = pubsub_relation_manager_repository
        .get_upstream_subscribe_id(
            subscribe_message.track_namespace().clone(),
            subscribe_message.track_name().to_string(),
            upstream_session_id,
        )
        .await?
        .unwrap();

    let largest_group_id = match object_cache_storage
        .get_largest_group_id(upstream_session_id, upstream_subscribe_id)
        .await
    {
        Ok(group_id) => Some(group_id),
        Err(_) => None,
    };

    // The largest object_id is None if the largest_group_id is None
    let largest_object_id = if let Some(group_id) = largest_group_id {
        match object_cache_storage
            .get_largest_object_id(upstream_session_id, upstream_subscribe_id, group_id)
            .await
        {
            Ok(object_id) => Some(object_id),
            Err(_) => None,
        }
    } else {
        None
    };

    // TODO: check cache duration
    let expires = 0;
    // If the largest_group_id or largest_object_id is None, the content does not exist
    let content_exists = largest_group_id.is_some() && largest_object_id.is_some();
    // TODO: check DELIVERY TIMEOUT
    let subscribe_parameters = vec![];
    // TODO: accurate group_order
    let group_order = subscribe_message.group_order();

    let subscribe_ok_message = SubscribeOk::new(
        subscribe_message.subscribe_id(),
        expires,
        group_order,
        content_exists,
        largest_group_id,
        largest_object_id,
        subscribe_parameters,
    );

    Ok(subscribe_ok_message)
}

async fn set_downstream_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    subscribe_message: &Subscribe,
    client: &MOQTClient,
) -> Result<()> {
    let downstream_client_id = client.id();
    let downstream_subscribe_id = subscribe_message.subscribe_id();
    let downstream_track_alias = subscribe_message.track_alias();
    let downstream_track_namespace = subscribe_message.track_namespace().to_vec();
    let downstream_track_name = subscribe_message.track_name().to_string();
    let subscriber_priority = subscribe_message.subscriber_priority();
    let downstream_group_order = subscribe_message.group_order();
    let downstream_filter_type = subscribe_message.filter_type();
    let downstream_start_group = subscribe_message.start_group();
    let downstream_start_object = subscribe_message.start_object();
    let downstream_end_group = subscribe_message.end_group();
    let downstream_end_object = subscribe_message.end_object();

    // Get publisher subscription already exists
    let upstream_subscription = pubsub_relation_manager_repository
        .get_upstream_subscription_by_full_track_name(
            downstream_track_namespace.clone(),
            downstream_track_name.clone(),
        )
        .await?
        .unwrap();

    pubsub_relation_manager_repository
        .set_downstream_subscription(
            downstream_client_id,
            downstream_subscribe_id,
            downstream_track_alias,
            downstream_track_namespace.clone(),
            downstream_track_name.clone(),
            subscriber_priority,
            downstream_group_order,
            downstream_filter_type,
            downstream_start_group,
            downstream_start_object,
            downstream_end_group,
            downstream_end_object,
        )
        .await?;

    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(downstream_track_namespace)
        .await?
        .unwrap();

    let (upstream_track_namespace, upstream_track_name) =
        upstream_subscription.get_track_namespace_and_name();

    // Get publisher subscribe id to register pubsub relation
    let upstream_subscribe_id = pubsub_relation_manager_repository
        .get_upstream_subscribe_id(
            upstream_track_namespace,
            upstream_track_name,
            upstream_session_id,
        )
        .await?
        .unwrap();

    pubsub_relation_manager_repository
        .set_pubsub_relation(
            upstream_session_id,
            upstream_subscribe_id,
            downstream_client_id,
            downstream_subscribe_id,
        )
        .await?;

    pubsub_relation_manager_repository
        .activate_downstream_subscription(downstream_client_id, downstream_subscribe_id)
        .await?;

    Ok(())
}

async fn set_downstream_and_upstream_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    subscribe_message: &Subscribe,
    client: &MOQTClient,
    upstream_session_id: usize,
) -> Result<(u64, u64)> {
    let downstream_client_id = client.id();
    let downstream_subscribe_id = subscribe_message.subscribe_id();
    let downstream_track_alias = subscribe_message.track_alias();
    let downstream_track_namespace = subscribe_message.track_namespace().to_vec();
    let downstream_track_name = subscribe_message.track_name().to_string();
    let subscriber_priority = subscribe_message.subscriber_priority();
    let downstream_group_order = subscribe_message.group_order();
    let downstream_filter_type = subscribe_message.filter_type();
    let downstream_start_group = subscribe_message.start_group();
    let downstream_start_object = subscribe_message.start_object();
    let downstream_end_group = subscribe_message.end_group();
    let downstream_end_object = subscribe_message.end_object();

    pubsub_relation_manager_repository
        .set_downstream_subscription(
            downstream_client_id,
            downstream_subscribe_id,
            downstream_track_alias,
            downstream_track_namespace.clone(),
            downstream_track_name.clone(),
            subscriber_priority,
            downstream_group_order,
            downstream_filter_type,
            downstream_start_group,
            downstream_start_object,
            downstream_end_group,
            downstream_end_object,
        )
        .await?;

    let (upstream_subscribe_id, upstream_track_alias) = pubsub_relation_manager_repository
        .set_upstream_subscription(
            upstream_session_id,
            downstream_track_namespace.clone(),
            downstream_track_name.clone(),
            subscriber_priority,
            downstream_group_order,
            downstream_filter_type,
            downstream_start_group,
            downstream_start_object,
            downstream_end_group,
            downstream_end_object,
        )
        .await?;

    pubsub_relation_manager_repository
        .set_pubsub_relation(
            upstream_session_id,
            upstream_subscribe_id,
            downstream_client_id,
            downstream_subscribe_id,
        )
        .await?;

    Ok((upstream_subscribe_id, upstream_track_alias))
}

#[cfg(test)]
mod success {
    use super::subscribe_handler;
    use crate::modules::{
        moqt_client::MOQTClient,
        object_cache_storage::{
            object_cache_storage, CacheHeader, CacheObject, ObjectCacheStorageCommand,
            ObjectCacheStorageWrapper,
        },
        pubsub_relation_manager::{
            commands::PubSubRelationCommand,
            manager::pubsub_relation_manager,
            wrapper::{test_helper_fn, PubSubRelationManagerWrapper},
        },
        send_stream_dispatcher::{
            send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
        },
    };
    use crate::SenderToOpenSubscription;
    use moqt_core::{
        constants::StreamDirection,
        data_stream_type::DataStreamType,
        messages::{
            control_messages::{
                subscribe::{FilterType, GroupOrder, Subscribe},
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            data_streams::object_stream_track::ObjectStreamTrack,
            data_streams::stream_header_track::StreamHeaderTrack,
            moqt_payload::MOQTPayload,
        },
        pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    };
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::{mpsc, Mutex};

    #[tokio::test]
    async fn normal_case_track_not_exists() {
        // Generate SUBSCRIBE message
        let expected_upstream_subscribe_id = 0;
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
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
            subscribe_parameters,
        )
        .unwrap();

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        // Register the publisher track in advance
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_ok());

        let message = result.unwrap();
        assert!(message.is_none());

        // Check the subscriber is registered
        let (_, producers, pubsub_relation) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        assert_eq!(producers.len(), 1);

        println!("{:?}", pubsub_relation);

        let subscribers = pubsub_relation
            .get_subscribers(upstream_session_id, expected_upstream_subscribe_id)
            .unwrap();

        let (downstream_session_id, downstream_subscribe_id) = subscribers.first().unwrap();

        assert_eq!(downstream_session_id, downstream_session_id);
        assert_eq!(downstream_subscribe_id, downstream_subscribe_id);
    }

    #[tokio::test]
    async fn normal_case_track_exists_and_content_not_exists() {
        // Generate SUBSCRIBE message
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
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
            subscribe_parameters,
        )
        .unwrap();

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        // Register the publisher track in advance
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
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
            )
            .await
            .unwrap();

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx.clone(),
            })
            .await;
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: downstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_ok());

        let message = result.unwrap();
        assert!(message.is_none());

        // Check the subscriber is registered
        let (_, producers, pubsub_relation) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        assert_eq!(producers.len(), 1);

        let subscribers = pubsub_relation
            .get_subscribers(upstream_session_id, upstream_subscribe_id)
            .unwrap();

        let (downstream_session_id, downstream_subscribe_id) = subscribers.first().unwrap();

        assert_eq!(downstream_session_id, downstream_session_id);
        assert_eq!(downstream_subscribe_id, downstream_subscribe_id);
    }

    #[tokio::test]
    async fn normal_case_track_exists_and_content_exists() {
        // Generate SUBSCRIBE message
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
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
            subscribe_parameters,
        )
        .unwrap();

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        // Register the publisher track in advance
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager
            .set_upstream_subscription(
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
            )
            .await
            .unwrap();

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx.clone(),
            })
            .await;
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: downstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let group_id = 0;
        let object_status = None;
        let duration = 1000;
        let publisher_priority = 0;

        let header = CacheHeader::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        let _ = object_cache_storage
            .set_subscription(upstream_session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Track(track.clone());

            let _ = object_cache_storage
                .set_object(upstream_session_id, subscribe_id, cache_object, duration)
                .await;
        }

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let (open_downstream_subscription_tx, mut open_downstream_subscription_rx) =
            mpsc::channel::<(u64, DataStreamType)>(32);
        open_downstream_subscription_txes
            .lock()
            .await
            .insert(downstream_session_id, open_downstream_subscription_tx);

        tokio::spawn(async move {
            let _ = open_downstream_subscription_rx.recv().await;
        });

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_ok());

        let message = result.unwrap();
        assert!(message.is_none());

        // Check the subscriber is registered
        let (_, producers, pubsub_relation) =
            test_helper_fn::get_node_and_relation_clone(&pubsub_relation_manager).await;

        assert_eq!(producers.len(), 1);

        let subscribers = pubsub_relation
            .get_subscribers(upstream_session_id, upstream_subscribe_id)
            .unwrap();

        let (downstream_session_id, downstream_subscribe_id) = subscribers.first().unwrap();

        assert_eq!(downstream_session_id, downstream_session_id);
        assert_eq!(downstream_subscribe_id, downstream_subscribe_id);
    }
}

#[cfg(test)]
mod failure {
    use super::subscribe_handler;
    use crate::modules::{
        moqt_client::MOQTClient,
        object_cache_storage::{
            object_cache_storage, ObjectCacheStorageCommand, ObjectCacheStorageWrapper,
        },
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
            wrapper::PubSubRelationManagerWrapper,
        },
        send_stream_dispatcher::{
            send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
        },
    };
    use crate::SenderToOpenSubscription;
    use moqt_core::{
        constants::StreamDirection,
        messages::{
            control_messages::{
                subscribe::{FilterType, GroupOrder, Subscribe},
                subscribe_error::SubscribeErrorCode,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_payload::MOQTPayload,
        },
        pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    };
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::{mpsc, Mutex};

    #[tokio::test]
    async fn cannot_register() {
        // Generate SUBSCRIBE message
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
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
            subscribe_parameters,
        )
        .unwrap();

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper (register subscriber in advance)
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        // Too Meny Subscribers
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn relay_fail() {
        // Generate SUBSCRIBE message
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
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
            subscribe_parameters,
        )
        .unwrap();

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_ok());

        if let Some(subscribe_error) = result.unwrap() {
            assert_eq!(
                subscribe_error.error_code(),
                SubscribeErrorCode::InternalError
            );
        }
    }

    #[tokio::test]
    async fn publisher_not_found() {
        // Generate SUBSCRIBE message
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name";
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name.to_string(),
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper (without set publisher)
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_ok());

        if let Some(subscribe_error) = result.unwrap() {
            assert_eq!(
                subscribe_error.error_code(),
                SubscribeErrorCode::TrackDoesNotExist
            );
        }
    }

    #[tokio::test]
    async fn too_many_subscriber() {
        // Generate SUBSCRIBE message
        let subscribe_ids = [0, 1];
        let track_aliases = [0, 1];
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let mut subscribes = vec![];

        for i in [0, 1] {
            let subscribe = Subscribe::new(
                subscribe_ids[i],
                track_aliases[i],
                track_namespace.clone(),
                track_name.clone(),
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
                subscribe_parameters.clone(),
            )
            .unwrap();

            subscribes.push(subscribe);
        }

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 0;

        // Register the publisher track in advance
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let _ = subscribe_handler(
            subscribes[0].clone(),
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes.clone(),
        )
        .await;

        let result = subscribe_handler(
            subscribes[1].clone(),
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn duplicate_track_alias() {
        // Generate SUBSCRIBE message
        let subscribe_ids = [0, 1];
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let mut subscribes = vec![];

        for i in [0, 1] {
            let subscribe = Subscribe::new(
                subscribe_ids[i],
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
                subscribe_parameters.clone(),
            )
            .unwrap();

            subscribes.push(subscribe);
        }

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let upstream_session_id = 1;
        let max_subscribe_id = 10;

        // Register the publisher track in advance
        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare open subscription sender
        let open_downstream_subscription_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Execute subscribe_handler and get result
        let _ = subscribe_handler(
            subscribes[0].clone(),
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes.clone(),
        )
        .await;

        let result = subscribe_handler(
            subscribes[1].clone(),
            &client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut object_cache_storage,
            open_downstream_subscription_txes,
        )
        .await;

        assert!(result.is_ok());

        if let Some(subscribe_error) = result.unwrap() {
            assert_eq!(
                subscribe_error.error_code(),
                SubscribeErrorCode::RetryTrackAlias
            );
        }
    }
}
