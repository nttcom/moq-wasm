use crate::modules::handlers::subscribe_handler::subscribe_handler;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::{moqt_payload::MOQTPayload, subscribe::Subscribe},
    moqt_client::MOQTClientStatus,
    MOQTClient, SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
};

pub(crate) async fn process_subscribe_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }

    let subscribe_request_message = match Subscribe::depacketize(payload_buf) {
        Ok(subscribe_request_message) => subscribe_request_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    subscribe_handler(
        subscribe_request_message,
        client,
        track_namespace_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await
}
