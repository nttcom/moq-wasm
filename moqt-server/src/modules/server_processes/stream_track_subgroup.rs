use crate::modules::handlers::stream_subgroup_header_handler::stream_header_subgroup_handler;
use crate::modules::object_cache_storage::ObjectCacheStorageWrapper;
use anyhow::{bail, Result};
use moqt_core::messages::data_streams::stream_header_subgroup::StreamHeaderSubgroup;
use moqt_core::messages::data_streams::DataStreams;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::MOQTClient;

pub(crate) async fn process_stream_header_subgroup(
    read_cur: &mut std::io::Cursor<&[u8]>,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    client: &mut MOQTClient,
) -> Result<u64> {
    let stream_header_subgroup = match StreamHeaderSubgroup::depacketize(read_cur) {
        Ok(stream_header_subgroup) => stream_header_subgroup,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };
    stream_header_subgroup_handler(
        stream_header_subgroup,
        pubsub_relation_manager_repository,
        object_cache_storage,
        client,
    )
    .await
}
