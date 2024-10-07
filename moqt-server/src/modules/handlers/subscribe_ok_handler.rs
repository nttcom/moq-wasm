use anyhow::Result;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    constants::StreamDirection,
    messages::{control_messages::subscribe_ok::SubscribeOk, moqt_payload::MOQTPayload},
    MOQTClient, SendStreamDispatcherRepository,
};

pub(crate) async fn subscribe_ok_handler(
    subscribe_ok_message: SubscribeOk,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    client: &mut MOQTClient,
) -> Result<()> {
    tracing::trace!("subscribe_ok_handler start.");

    tracing::debug!("subscribe_ok_message: {:#?}", subscribe_ok_message);

    let publisher_session_id = client.id;
    let publisher_subscribe_id = subscribe_ok_message.subscribe_id();

    // Determine the SUBSCRIBER who sent the SUBSCRIBE using the track_namespace and track_name
    let ids = pubsub_relation_manager_repository
        .get_requesting_subscriber_session_ids_and_subscribe_ids(
            publisher_subscribe_id,
            publisher_session_id,
        )
        .await?;
    match ids {
        Some(ids) => {
            let _ = pubsub_relation_manager_repository
                .activate_publisher_subscription(publisher_session_id, publisher_subscribe_id)
                .await;

            // Notify all waiting subscribers with the SUBSCRIBE_OK message
            for (subscriber_session_id, subscriber_subscribe_id) in ids.iter() {
                let mut relaying_subscribe_ok_message = subscribe_ok_message.clone();
                relaying_subscribe_ok_message.replace_subscribe_id(*subscriber_subscribe_id);
                let message: Box<dyn MOQTPayload> = Box::new(relaying_subscribe_ok_message.clone());

                tracing::debug!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    relaying_subscribe_ok_message,
                    subscriber_session_id
                );

                send_stream_dispatcher_repository
                    .send_message_to_send_stream_thread(
                        *subscriber_session_id,
                        message,
                        StreamDirection::Bi,
                    )
                    .await?;

                pubsub_relation_manager_repository
                    .activate_subscriber_subscription(
                        *subscriber_session_id,
                        *subscriber_subscribe_id,
                    )
                    .await?;

                tracing::trace!("subscribe_ok_handler complete.");
            }
        }
        None => {
            tracing::warn!("waiting subscriber session ids not found");

            // Activate the publisher because it is in the Requesting state
            let _ = pubsub_relation_manager_repository
                .activate_publisher_subscription(publisher_session_id, publisher_subscribe_id)
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::subscribe_ok_handler::subscribe_ok_handler;
    use crate::modules::relation_manager::{
        commands::PubSubRelationCommand, interface::PubSubRelationManagerInterface,
        manager::pubsub_relation_manager,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::MOQTClient;
    use moqt_core::constants::StreamDirection;
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::messages::control_messages::version_specific_parameters::{
        AuthorizationInfo, VersionSpecificParameter,
    };
    use moqt_core::messages::{
        control_messages::subscribe_ok::SubscribeOk, moqt_payload::MOQTPayload,
    };
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate SUBSCRIBE_OK message
        let subscriber_subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscriber_subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 1;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let subscriber_session_id = 2;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = pubsub_relation_manager
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

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_subscriber_subscription(
                subscriber_session_id,
                subscriber_subscribe_id,
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
        let _ = pubsub_relation_manager
            .register_pubsup_relation(
                publisher_session_id,
                publisher_subscribe_id,
                subscriber_session_id,
                subscriber_subscribe_id,
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
                session_id: subscriber_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut client,
        )
        .await;

        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::handlers::subscribe_ok_handler::subscribe_ok_handler;
    use crate::modules::relation_manager::{
        commands::PubSubRelationCommand, interface::PubSubRelationManagerInterface,
        manager::pubsub_relation_manager,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::MOQTClient;
    use moqt_core::constants::StreamDirection;
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::messages::control_messages::version_specific_parameters::{
        AuthorizationInfo, VersionSpecificParameter,
    };
    use moqt_core::messages::{
        control_messages::subscribe_ok::SubscribeOk, moqt_payload::MOQTPayload,
    };
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn relay_fail() {
        // Generate SUBSCRIBE_OK message
        let subscriber_subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscriber_subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 1;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let subscriber_session_id = 2;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = pubsub_relation_manager
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

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_subscriber_subscription(
                subscriber_session_id,
                subscriber_subscribe_id,
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
        let _ = pubsub_relation_manager
            .register_pubsup_relation(
                publisher_session_id,
                publisher_subscribe_id,
                subscriber_session_id,
                subscriber_subscribe_id,
            )
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut client,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn subscriber_not_found() {
        // Generate SUBSCRIBE_OK message
        let subscriber_subscribe_id = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscriber_subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 1;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let subscriber_session_id = 2;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = pubsub_relation_manager
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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: subscriber_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut client,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn subscriber_already_activated() {
        // Generate SUBSCRIBE_OK message
        let subscriber_subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscriber_subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 1;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let subscriber_session_id = 2;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = pubsub_relation_manager
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

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_subscriber_subscription(
                subscriber_session_id,
                subscriber_subscribe_id,
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
        let _ = pubsub_relation_manager
            .register_pubsup_relation(
                publisher_session_id,
                publisher_subscribe_id,
                subscriber_session_id,
                subscriber_subscribe_id,
            )
            .await;

        let _ = pubsub_relation_manager
            .activate_subscriber_subscription(subscriber_session_id, subscriber_subscribe_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: subscriber_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut client,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn publisher_already_activated() {
        // Generate SUBSCRIBE_OK message
        let subscriber_subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscriber_subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 1;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let subscriber_session_id = 2;
        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_publisher_announced_namespace(track_namespace.clone(), publisher_session_id)
            .await;
        let (publisher_subscribe_id, _) = pubsub_relation_manager
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

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, subscriber_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_subscriber_subscription(
                subscriber_session_id,
                subscriber_subscribe_id,
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
        let _ = pubsub_relation_manager
            .register_pubsup_relation(
                publisher_session_id,
                publisher_subscribe_id,
                subscriber_session_id,
                subscriber_subscribe_id,
            )
            .await;

        let _ = pubsub_relation_manager
            .activate_publisher_subscription(publisher_session_id, publisher_subscribe_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: subscriber_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut client,
        )
        .await;

        assert!(result.is_ok());
    }
}
