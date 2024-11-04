use crate::SenderToOpenSubscription;
use anyhow::{bail, Result};
use moqt_core::{
    constants::StreamDirection,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::{
            fetch::Fetch,
            fetch_error::{FetchError, FetchErrorCode},
            fetch_ok::FetchOk,
        },
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    MOQTClient, SendStreamDispatcherRepository,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::modules::object_cache_storage::{CacheHeader, ObjectCacheStorageWrapper};

pub(crate) async fn fetch_handler(
    fetch_message: Fetch,
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
) -> Result<Option<FetchError>> {
    tracing::trace!("fetch_handler start.");

    tracing::debug!("fetch_message: {:#?}", fetch_message);

    // TODO: validate Unauthorized

    if !pubsub_relation_manager_repository
        .is_valid_downstream_subscribe_id(fetch_message.subscribe_id(), client.id)
        .await?
    {
        // TODO: return TerminationErrorCode
        bail!("TooManyFetchrs");
    }
    if !pubsub_relation_manager_repository
        .is_valid_downstream_track_alias(fetch_message.track_alias(), client.id)
        .await?
    {
        // TODO: create accurate track alias
        let reason_phrase = "Invalid Track Alias".to_string();
        let fetch_error = FetchError::new(
            fetch_message.subscribe_id(),
            FetchErrorCode::RetryTrackAlias,
            reason_phrase,
            100, // track alias
        );
        return Ok(Some(fetch_error));

        // TODO: return TerminationErrorCode::DuplicateTrackAlias
    }

    // TODO: validate Invalid Range

    // If the track exists, return ther track as it is
    if pubsub_relation_manager_repository
        .is_track_existing(
            fetch_message.track_namespace().to_vec(),
            fetch_message.track_name().to_string(),
        )
        .await
        .unwrap()
    {
        // Generate message -> Set subscription -> Send message
        let fetch_ok_message = match generate_fetch_ok_message(
            pubsub_relation_manager_repository,
            object_cache_storage,
            &fetch_message,
        )
        .await
        {
            Ok(message) => {
                match set_downstream_subscription(
                    pubsub_relation_manager_repository,
                    &fetch_message,
                    client,
                )
                .await
                {
                    Ok(_) => {
                        tracing::info!(
                            "fetchd track_namespace: {:?}",
                            fetch_message.track_namespace(),
                        );
                        tracing::info!("fetchd track_name: {:?}", fetch_message.track_name());
                        tracing::trace!("fetch_handler complete.");
                    }
                    Err(e) => {
                        let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                        let fetch_error = FetchError::new(
                            fetch_message.subscribe_id(),
                            FetchErrorCode::InternalError,
                            reason_phrase,
                            fetch_message.track_alias(),
                        );
                        return Ok(Some(fetch_error));
                    }
                }
                message
            }
            Err(e) => {
                let reason_phrase = "InternalError: ".to_string() + &e.to_string();
                let fetch_error = FetchError::new(
                    fetch_message.subscribe_id(),
                    FetchErrorCode::InternalError,
                    reason_phrase,
                    fetch_message.track_alias(),
                );
                return Ok(Some(fetch_error));
            }
        };

        // Send SUBSCRIBE_OK message if generate massage and set subscription is successfully done
        let fetch_ok_payload: Box<dyn MOQTPayload> = Box::new(fetch_ok_message.clone());

        // TODO: Unify the method to send a message to the opposite client itself
        send_stream_dispatcher_repository
            .send_message_to_send_stream_thread(client.id, fetch_ok_payload, StreamDirection::Bi)
            .await?;

        if fetch_ok_message.content_exists() {
            open_new_subscription(
                pubsub_relation_manager_repository,
                object_cache_storage,
                open_subscription_txes,
                client,
                fetch_message,
            )
            .await?;
        }

        return Ok(None);
    }
}

async fn open_new_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
    client: &MOQTClient,
    fetch_message: Fetch,
) -> Result<()> {
    let downstream_session_id = client.id;
    let downstream_subscribe_id = fetch_message.subscribe_id();

    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(fetch_message.track_namespace().clone())
        .await?
        .unwrap();

    let upstream_subscribe_id = pubsub_relation_manager_repository
        .get_upstream_subscribe_id(
            fetch_message.track_namespace().clone(),
            fetch_message.track_name().to_string(),
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

    let open_subscription_tx = open_subscription_txes
        .lock()
        .await
        .get(&downstream_session_id)
        .unwrap()
        .clone();

    let _ = open_subscription_tx
        .send((downstream_subscribe_id, stream_header_type.clone()))
        .await;

    Ok(())
}

async fn generate_fetch_ok_message(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    fetch_message: &Fetch,
) -> Result<FetchOk> {
    let upstream_session_id = pubsub_relation_manager_repository
        .get_upstream_session_id(fetch_message.track_namespace().clone())
        .await?
        .unwrap();

    let upstream_subscribe_id = pubsub_relation_manager_repository
        .get_upstream_subscribe_id(
            fetch_message.track_namespace().clone(),
            fetch_message.track_name().to_string(),
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
    let fetch_parameters = vec![];
    // TODO: accurate group_order
    let group_order = fetch_message.group_order();

    let fetch_ok_message = FetchOk::new(
        fetch_message.subscribe_id(),
        expires,
        group_order,
        content_exists,
        largest_group_id,
        largest_object_id,
        fetch_parameters,
    );

    Ok(fetch_ok_message)
}

async fn set_downstream_subscription(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    fetch_message: &Fetch,
    client: &MOQTClient,
) -> Result<()> {
    let downstream_client_id = client.id;
    let downstream_subscribe_id = fetch_message.subscribe_id();
    let downstream_track_alias = fetch_message.track_alias();
    let downstream_track_namespace = fetch_message.track_namespace().to_vec();
    let downstream_track_name = fetch_message.track_name().to_string();
    let fetchr_priority = fetch_message.fetchr_priority();
    let downstream_group_order = fetch_message.group_order();
    let downstream_filter_type = fetch_message.filter_type();
    let downstream_start_group = fetch_message.start_group();
    let downstream_start_object = fetch_message.start_object();
    let downstream_end_group = fetch_message.end_group();
    let downstream_end_object = fetch_message.end_object();

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
            fetchr_priority,
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

    // Get publisher fetch id to register pubsub relation
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
