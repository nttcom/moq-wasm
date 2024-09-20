use crate::modules::handlers::object_handler::{
    object_with_payload_length_handler, object_without_payload_length_handler,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::{
        moqt_payload::MOQTPayload,
        object::{ObjectWithPayloadLength, ObjectWithoutPayloadLength},
    },
    SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
};

pub(crate) async fn process_object_with_payload_length(
    payload_buf: &mut BytesMut,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    let object_message = match ObjectWithPayloadLength::depacketize(payload_buf) {
        Ok(object_message) => object_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    object_with_payload_length_handler(
        object_message,
        track_namespace_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await
}

pub(crate) async fn process_object_without_payload_length(
    payload_buf: &mut BytesMut,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    let object_message = match ObjectWithoutPayloadLength::depacketize(payload_buf) {
        Ok(object_message) => object_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    object_without_payload_length_handler(
        object_message,
        track_namespace_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await
}
