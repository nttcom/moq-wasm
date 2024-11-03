use anyhow::{bail, Result};
use moqt_core::{
    constants::StreamDirection,
    messages::{data_streams::object_datagram::ObjectDatagram, moqt_payload::MOQTPayload},
    models::tracks::ForwardingPreference,
    MOQTClient, PubSubRelationManagerRepository, SendStreamDispatcherRepository,
};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::modules::object_cache_storage::ObjectCacheStorageWrapper;

use super::object_cache_storage::CacheHeader;

pub async fn datagram_handler(
    upstream_session_id: usize,
    object_datagram: ObjectDatagram,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
) -> Result<()> {
    let upstream_session_id: usize;
    {
        upstream_session_id = client.lock().await.id();
    }
    let upstream_subscribe_id = object_datagram.subscribe_id();

    match object_cache_storage
        .get_header(upstream_session_id, upstream_subscribe_id)
        .await
    {
        Ok(CacheHeader::Datagram) => {}
        // It's first time to receive datagram
        Err(e) => {
            pubsub_relation_manager_repository
                .set_upstream_forwarding_preference(
                    upstream_session_id,
                    upstream_subscribe_id,
                    ForwardingPreference::Datagram,
                )
                .await?;

            object_cache_storage
                .set_subscription(
                    upstream_session_id,
                    upstream_subscribe_id,
                    CacheHeader::Datagram,
                )
                .await?;
        }
        _ => {
            let msg = "failed to get cache header, error: unexpected cache header is already set"
                .to_string();
            tracing::error!(msg);
            bail!(msg);
        }
    }

    let downstream_subscriptions = pubsub_relation_manager_repository
        .get_downstream_session_ids_and_subscribe_ids(upstream_session_id, upstream_subscribe_id)
        .await
        .unwrap();

    for (downstream_session_id, downstream_subscribe_id) in downstream_subscriptions {
        let downstream_object_datagram: Box<dyn MOQTPayload> = Box::new(
            ObjectDatagram::new(
                downstream_subscribe_id,
                object_datagram.track_alias(),
                object_datagram.group_id(),
                object_datagram.object_id(),
                object_datagram.publisher_priority(),
                object_datagram.object_status(),
                object_datagram.object_payload(),
            )
            .unwrap(),
        );

        match send_stream_dispatcher_repository
            .send_message_to_send_stream_thread(
                downstream_session_id,
                downstream_object_datagram,
                StreamDirection::Datagram,
            )
            .await
        {
            Ok(_) => {
                tracing::info!(
                    "datagram relayed to downstream session_id: {}",
                    downstream_session_id
                )
            }
            Err(e) => {
                tracing::error!(
                    "failed to relay datagram to downstream session_id: {}, error: {}",
                    downstream_session_id,
                    e
                )
            }
        }
    }

    Ok(())
}
