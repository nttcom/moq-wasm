use anyhow::{bail, Result};

use moqt_core::{
    constants::StreamDirection,
    messages::{control_messages::subscribe_error::SubscribeError, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    SendStreamDispatcherRepository,
};

use crate::modules::moqt_client::MOQTClient;

pub(crate) async fn subscribe_error_handler(
    subscribe_error_message: SubscribeError,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    client: &mut MOQTClient,
) -> Result<()> {
    tracing::trace!("subscribe_error_handler start.");

    tracing::debug!("subscribe_error_message: {:#?}", subscribe_error_message);

    let upstream_session_id = client.id();
    let upstream_subscribe_id = subscribe_error_message.subscribe_id();

    // TODO: Retry to send the SUBSCRIBE message if the error type is Retry Track Alias
    // Determine the SUBSCRIBER who sent the SUBSCRIBE using the track_namespace and track_name
    let downstream_ids = pubsub_relation_manager_repository
        .get_requesting_downstream_session_ids_and_subscribe_ids(
            upstream_subscribe_id,
            upstream_session_id,
        )
        .await?;
    match downstream_ids {
        Some(downstream_ids) => {
            // Notify all waiting subscribers with the subscribe_error message
            // And delete subscripsions and relation
            for (downstream_session_id, downstream_subscribe_id) in downstream_ids.iter() {
                match delete_downstream_and_upstream_subscription(
                    pubsub_relation_manager_repository,
                    upstream_session_id,
                    upstream_subscribe_id,
                    *downstream_session_id,
                    *downstream_subscribe_id,
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        bail!("cannot delete publisher and subscriber: {:?}", e);
                    }
                }

                // TODO: set downstream_track_alias if the error type is other than Retry Track Alias

                let message_payload = SubscribeError::new(
                    *downstream_subscribe_id,
                    subscribe_error_message.error_code(),
                    subscribe_error_message.reason_phrase().to_string(),
                    subscribe_error_message.track_alias(),
                );
                let relaying_subscribe_error_message: Box<dyn MOQTPayload> =
                    Box::new(message_payload.clone());

                send_stream_dispatcher_repository
                    .transfer_message_to_send_stream_thread(
                        *downstream_session_id,
                        relaying_subscribe_error_message,
                        StreamDirection::Bi,
                    )
                    .await?;

                tracing::debug!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    message_payload,
                    downstream_session_id
                );

                tracing::trace!("subscribe_error_handler complete.");
            }
        }
        None => {
            tracing::warn!("waiting subscriber session ids not found");
        }
    }

    Ok(())
}

async fn delete_downstream_and_upstream_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    upstream_session_id: usize,
    upstream_subscribe_id: u64,
    downstream_session_id: usize,
    downstream_subscribe_id: u64,
) -> Result<()> {
    pubsub_relation_manager_repository
        .delete_pubsub_relation(
            upstream_session_id,
            upstream_subscribe_id,
            downstream_session_id,
            downstream_subscribe_id,
        )
        .await?;

    pubsub_relation_manager_repository
        .delete_upstream_subscription(upstream_session_id, upstream_subscribe_id)
        .await?;

    pubsub_relation_manager_repository
        .delete_downstream_subscription(downstream_session_id, downstream_subscribe_id)
        .await?;

    Ok(())
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::subscribe_error_handler::subscribe_error_handler;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::MOQTClient;
    use moqt_core::constants::StreamDirection;
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::messages::{
        control_messages::subscribe_error::{SubscribeError, SubscribeErrorCode},
        moqt_payload::MOQTPayload,
    };
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate subscribe_error message
        let downstream_subscribe_id = 0;
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

        // Generate client
        let upstream_session_id = 1;
        let mut client = MOQTClient::new(upstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let downstream_session_id = 2;
        let max_subscribe_id = 10;

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
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                downstream_subscribe_id,
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
            .set_pubsub_relation(
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
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
                session_id: downstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        let error_code = SubscribeErrorCode::InternalError;
        let reason_phrase = "test".to_string();
        let track_alias = 20;

        let subscribe_error = SubscribeError::new(
            upstream_subscribe_id,
            error_code,
            reason_phrase,
            track_alias,
        );

        // Execute subscribe_error_handler and get result
        let result = subscribe_error_handler(
            subscribe_error,
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
    use crate::modules::handlers::subscribe_error_handler::subscribe_error_handler;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::MOQTClient;
    use moqt_core::messages::control_messages::subscribe::{FilterType, GroupOrder};
    use moqt_core::messages::control_messages::subscribe_error::{
        SubscribeError, SubscribeErrorCode,
    };
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn relay_fail() {
        // Generate subscribe_error message
        let downstream_subscribe_id = 0;
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

        // Generate client
        let upstream_session_id = 1;
        let mut client = MOQTClient::new(upstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let downstream_session_id = 2;
        let max_subscribe_id = 10;

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
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .set_downstream_subscription(
                downstream_session_id,
                downstream_subscribe_id,
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
            .set_pubsub_relation(
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
            )
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let error_code = SubscribeErrorCode::InternalError;
        let reason_phrase = "test".to_string();
        let track_alias = 20;

        let subscribe_error = SubscribeError::new(
            upstream_subscribe_id,
            error_code,
            reason_phrase,
            track_alias,
        );

        // Execute subscribe_error_handler and get result
        let result = subscribe_error_handler(
            subscribe_error,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
            &mut client,
        )
        .await;

        assert!(result.is_err());
    }
}
