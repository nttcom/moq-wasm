use anyhow::{bail, Result};

use moqt_core::{
    messages::data_streams::{stream_header_subgroup::StreamHeaderSubgroup, DataStreams},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::modules::{
    message_handlers::stream_header::handlers::stream_subgroup_header_handler::stream_header_subgroup_handler,
    moqt_client::MOQTClient, object_cache_storage::ObjectCacheStorageWrapper,
};

pub(crate) async fn process_stream_header_subgroup(
    read_cur: &mut std::io::Cursor<&[u8]>,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    client: &MOQTClient,
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
