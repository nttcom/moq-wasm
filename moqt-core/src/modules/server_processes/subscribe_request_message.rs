use crate::{
    handlers::subscribe_handler::subscribe_handler,
    messages::{moqt_payload::MOQTPayload, subscribe_request_message::SubscribeRequestMessage},
    moqt_client::MOQTClientStatus,
    MOQTClient, RelayHandlerManagerRepository, TrackNamespaceManagerRepository,
};
use anyhow::{bail, Result};
use bytes::BytesMut;

pub(crate) async fn process_subscribe_message(
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

    let subscribe_request_message = match SubscribeRequestMessage::depacketize(payload_buf) {
        Ok(subscribe_request_message) => subscribe_request_message,
        Err(err) => {
            tracing::info!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    subscribe_handler(
        subscribe_request_message,
        client,
        track_namespace_manager_repository,
        relay_handler_manager_repository,
    )
    .await
}
