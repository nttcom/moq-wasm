use crate::{
    handlers::object_handler::{
        object_with_payload_length_handler, object_without_payload_length_handler,
    },
    messages::{
        moqt_payload::MOQTPayload,
        object_message::{ObjectWithPayloadLength, ObjectWithoutPayloadLength},
    },
    moqt_client::MOQTClientStatus,
    MOQTClient, RelayHandlerManagerRepository, TrackNamespaceManagerRepository,
};
use anyhow::{bail, Result};
use bytes::BytesMut;

pub(crate) async fn process_object_with_payload_length(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    relay_handler_manager_repository: &mut dyn RelayHandlerManagerRepository,
) -> Result<()> {
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }

    let object_message = match ObjectWithPayloadLength::depacketize(payload_buf) {
        Ok(object_message) => object_message,
        Err(err) => {
            tracing::info!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    object_with_payload_length_handler(
        object_message,
        track_namespace_manager_repository,
        relay_handler_manager_repository,
    )
    .await
}

pub(crate) async fn process_object_without_payload_length(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    relay_handler_manager_repository: &mut dyn RelayHandlerManagerRepository,
) -> Result<()> {
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }

    let object_message = match ObjectWithoutPayloadLength::depacketize(payload_buf) {
        Ok(object_message) => object_message,
        Err(err) => {
            tracing::info!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    object_without_payload_length_handler(
        object_message,
        track_namespace_manager_repository,
        relay_handler_manager_repository,
    )
    .await
}
