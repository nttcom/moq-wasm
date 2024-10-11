use anyhow::Result;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    constants::StreamDirection,
    messages::{data_streams::stream_header_track::StreamHeaderTrack, moqt_payload::MOQTPayload},
    MOQTClient, SendStreamDispatcherRepository,
};

pub(crate) async fn stream_header_track_handler(
    stream_header_track_message: StreamHeaderTrack,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    client: &mut MOQTClient,
) -> Result<()> {
    tracing::trace!("stream_header_track_handler start.");

    tracing::debug!(
        "stream_header_track_message: {:#?}",
        stream_header_track_message
    );

    Ok(())
}
