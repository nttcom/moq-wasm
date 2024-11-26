use crate::modules::handlers::stream_track_header_handler::stream_header_track_handler;
use crate::modules::object_cache_storage::ObjectCacheStorageWrapper;
use anyhow::{bail, Result};
use moqt_core::messages::data_streams::stream_header_track::StreamHeaderTrack;
use moqt_core::messages::data_streams::DataStreams;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::MOQTClient;

pub(crate) async fn process_stream_header_track(
    read_cur: &mut std::io::Cursor<&[u8]>,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    client: &MOQTClient,
) -> Result<u64> {
    let stream_header_track = match StreamHeaderTrack::depacketize(read_cur) {
        Ok(stream_header_track) => stream_header_track,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };
    stream_header_track_handler(
        stream_header_track,
        pubsub_relation_manager_repository,
        object_cache_storage,
        client,
    )
    .await
}
