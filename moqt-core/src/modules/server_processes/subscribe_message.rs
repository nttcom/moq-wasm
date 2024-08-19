use crate::{
    handlers::subscribe_handler::{subscribe_handler, SubscribeResponse},
    messages::{moqt_payload::MOQTPayload, subscribe_request_message::SubscribeRequestMessage},
    moqt_client::MOQTClientStatus,
    MOQTClient, StreamManagerRepository, TrackNamespaceManagerRepository,
};
use anyhow::{bail, Result};
use bytes::BytesMut;

pub(crate) async fn process_subscribe_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    write_buf: &mut BytesMut,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    stream_manager_repository: &mut dyn StreamManagerRepository,
) -> Result<SubscribeResponse> {
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

    let subscribe_result = subscribe_handler(
        subscribe_request_message,
        client,
        track_namespace_manager_repository,
        stream_manager_repository,
    )
    .await;

    match subscribe_result {
        Ok(subscribe_response) => match subscribe_response {
            crate::modules::handlers::subscribe_handler::SubscribeResponse::Success(
                subscribe_ok,
            ) => {
                subscribe_ok.packetize(write_buf);
                Ok(SubscribeResponse::Success(subscribe_ok))
            }
            crate::modules::handlers::subscribe_handler::SubscribeResponse::Failure(
                subscribe_error,
            ) => {
                subscribe_error.packetize(write_buf);
                Ok(SubscribeResponse::Failure(subscribe_error))
            }
        },
        Err(err) => {
            // fix
            tracing::info!("{:#?}", err);
            bail!(err.to_string());
        }
    }
}
