use crate::{
    handlers::subscribe_ok_handler::subscribe_ok_handler,
    messages::{moqt_payload::MOQTPayload, subscribe_ok::SubscribeOk},
    moqt_client::MOQTClientStatus,
    MOQTClient, SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
};
use anyhow::{bail, Result};
use bytes::BytesMut;

pub(crate) async fn process_subscribe_ok_message(
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

    let subscribe_ok_message = match SubscribeOk::depacketize(payload_buf) {
        Ok(subscribe_ok_message) => subscribe_ok_message,
        Err(err) => {
            tracing::info!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    subscribe_ok_handler(
        subscribe_ok_message,
        track_namespace_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await
}
