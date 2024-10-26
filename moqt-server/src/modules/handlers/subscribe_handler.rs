use anyhow::{bail, Result};
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    constants::StreamDirection,
    messages::{
        control_messages::{
            subscribe::Subscribe,
            subscribe_error::{SubscribeError, SubscribeErrorCode},
            subscribe_ok::SubscribeOk,
        },
        moqt_payload::MOQTPayload,
    },
    MOQTClient, SendStreamDispatcherRepository,
};

#[derive(Debug, PartialEq)]
pub(crate) enum SubscribeResponse {
    Success(SubscribeOk),
    Failure(SubscribeError),
}

pub(crate) async fn subscribe_handler(
    subscribe_message: Subscribe,
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<Option<SubscribeResponse>> {
    tracing::trace!("subscribe_handler start.");

    tracing::debug!("subscribe_message: {:#?}", subscribe_message);

    // TODO: validate Unauthorized

    if !pubsub_relation_manager_repository
        .is_valid_downstream_subscribe_id(subscribe_message.subscribe_id(), client.id)
        .await?
    {
        // TODO: return TerminationErrorCode
        bail!("TooManySubscribers");
    }
    if !pubsub_relation_manager_repository
        .is_valid_downstream_track_alias(subscribe_message.track_alias(), client.id)
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

        return Ok(Some(SubscribeResponse::Failure(subscribe_error)));

        // TODO: return TerminationErrorCode::DuplicateTrackAlias
    }

    // If the track exists, return ther track as it is
    if pubsub_relation_manager_repository
        .is_track_existing(
            subscribe_message.track_namespace().to_vec(),
            subscribe_message.track_name().to_string(),
        )
        .await
        .unwrap()
    {
        match set_downstream_subscription(
            pubsub_relation_manager_repository,
            &subscribe_message,
            client,
        )
        .await
        {
            Ok(_) => {
                // Generate and return subscribe_ok message

                // TODO: Implement the get object info when implement cache mechanism
                // TODO: validate Invalid Range
                let expires = 0;
                let content_exist = false;
                let largest_group_id = None;
                let largest_object_id = None;
                let subscribe_parameters = vec![];

                let subscribe_ok = SubscribeOk::new(
                    subscribe_message.subscribe_id(),
                    expires,
                    subscribe_message.group_order(),
                    content_exist,
                    largest_group_id,
                    largest_object_id,
                    subscribe_parameters,
                );

                return Ok(Some(SubscribeResponse::Success(subscribe_ok)));
            }
            Err(e) => {
                let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                let subscribe_error = SubscribeError::new(
                    subscribe_message.subscribe_id(),
                    SubscribeErrorCode::InternalError,
                    reason_phrase,
                    subscribe_message.track_alias(),
                );
                return Ok(Some(SubscribeResponse::Failure(subscribe_error)));
            }
        }
    }

    // Since only the track_namespace is recorded in ANNOUNCE, use track_namespace to determine the publisher
    // TODO: multiple publishers for the same track_namespace
    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(subscribe_message.track_namespace().clone())
        .await
        .unwrap();
    match upstream_session_id {
        Some(session_id) => {
            let (upstream_subscribe_id, _) = match set_downstream_and_upstream_subscription(
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

                    return Ok(Some(SubscribeResponse::Failure(subscribe_error)));
                }
            };

            let mut relaying_subscribe_message = subscribe_message.clone();

            // Replace the subscribe_id and track_alias in the SUBSCRIBE message to request to the upstream publisher
            relaying_subscribe_message.set_subscribe_id(upstream_subscribe_id);
            relaying_subscribe_message.set_track_alias(upstream_subscribe_id);

            let message: Box<dyn MOQTPayload> = Box::new(relaying_subscribe_message.clone());

            tracing::debug!(
                "message: {:#?} is sent to relay handler for client {:?}",
                relaying_subscribe_message,
                session_id
            );

            // Notify to the publisher about the SUBSCRIBE message
            // TODO: Wait for the SUBSCRIBE_OK message to be returned on a transaction
            // TODO: validate Timeout
            match send_stream_dispatcher_repository
                .send_message_to_send_stream_thread(session_id, message, StreamDirection::Bi)
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "subscribed track_namespace: {:?}",
                        relaying_subscribe_message.track_namespace(),
                    );
                    tracing::info!(
                        "subscribed track_name: {:?}",
                        relaying_subscribe_message.track_name()
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

                    return Ok(Some(SubscribeResponse::Failure(subscribe_error)));
                }
            }

            Ok(None)
        }

        // TODO: Check if “publisher not found” should turn into closing connection
        None => {
            let reason_phrase = "publisher not found".to_string();
            let subscribe_error = SubscribeError::new(
                subscribe_message.subscribe_id(),
                SubscribeErrorCode::TrackDoesNotExist,
                reason_phrase,
                subscribe_message.track_alias(),
            );

            Ok(Some(SubscribeResponse::Failure(subscribe_error)))
        }
    }
}

async fn set_downstream_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    subscribe_message: &Subscribe,
    client: &MOQTClient,
) -> Result<()> {
    let downstream_client_id = client.id;
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
    let downstream_client_id = client.id;
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
    use crate::modules::handlers::subscribe_handler::subscribe_handler;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand,
        manager::pubsub_relation_manager,
        wrapper::{test_helper_fn, PubSubRelationManagerWrapper},
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use moqt_core::constants::StreamDirection;
    use moqt_core::messages::{
        control_messages::{
            subscribe::{FilterType, GroupOrder, Subscribe},
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        moqt_payload::MOQTPayload,
    };
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use moqt_core::MOQTClient;
    use std::sync::Arc;
    use tokio::sync::mpsc;

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
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
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
    async fn normal_case_track_exists() {
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
        let mut client = MOQTClient::new(downstream_session_id);

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
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());

        let message = result.unwrap();
        assert!(message.is_some());

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
    use crate::modules::handlers::subscribe_handler::subscribe_handler;
    use crate::modules::handlers::subscribe_handler::SubscribeResponse;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use moqt_core::constants::StreamDirection;

    use moqt_core::messages::control_messages::subscribe_error::SubscribeErrorCode;
    use moqt_core::messages::{
        control_messages::{
            subscribe::{FilterType, GroupOrder, Subscribe},
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        moqt_payload::MOQTPayload,
    };
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use moqt_core::MOQTClient;
    use std::sync::Arc;
    use tokio::sync::mpsc;

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
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
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
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());

        if let Some(SubscribeResponse::Failure(subscribe_error)) = result.unwrap() {
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
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());

        if let Some(SubscribeResponse::Failure(subscribe_error)) = result.unwrap() {
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
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Execute subscribe_handler and get result
        let _ = subscribe_handler(
            subscribes[0].clone(),
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        let result = subscribe_handler(
            subscribes[1].clone(),
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
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
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Execute subscribe_handler and get result
        let _ = subscribe_handler(
            subscribes[0].clone(),
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        let result = subscribe_handler(
            subscribes[1].clone(),
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());

        if let Some(SubscribeResponse::Failure(subscribe_error)) = result.unwrap() {
            assert_eq!(
                subscribe_error.error_code(),
                SubscribeErrorCode::RetryTrackAlias
            );
        }
    }
}
