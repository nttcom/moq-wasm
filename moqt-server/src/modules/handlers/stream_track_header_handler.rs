use crate::modules::{
    moqt_client::MOQTClient,
    object_cache_storage::{CacheHeader, ObjectCacheStorageWrapper},
};
use anyhow::Result;
use moqt_core::{
    messages::data_streams::stream_header_track::StreamHeaderTrack,
    models::tracks::ForwardingPreference,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

pub(crate) async fn stream_header_track_handler(
    stream_header_track_message: StreamHeaderTrack,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    client: &mut MOQTClient,
) -> Result<u64> {
    tracing::trace!("stream_header_track_handler start.");

    tracing::debug!(
        "stream_header_track_message: {:#?}",
        stream_header_track_message
    );

    let upstream_session_id = client.id();
    let upstream_subscribe_id = stream_header_track_message.subscribe_id();

    pubsub_relation_manager_repository
        .set_upstream_forwarding_preference(
            upstream_session_id,
            upstream_subscribe_id,
            ForwardingPreference::Track,
        )
        .await?;

    let cache_header = CacheHeader::Track(stream_header_track_message);
    object_cache_storage
        .set_subscription(upstream_session_id, upstream_subscribe_id, cache_header)
        .await?;

    Ok(upstream_subscribe_id)
}
