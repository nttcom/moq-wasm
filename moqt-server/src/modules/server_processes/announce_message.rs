use crate::modules::handlers::announce_handler::{announce_handler, AnnounceResponse};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::{announce::Announce, moqt_payload::MOQTPayload},
    moqt_client::MOQTClientStatus,
    MOQTClient, TrackNamespaceManagerRepository,
};

pub(crate) async fn process_announce_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    write_buf: &mut BytesMut,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
) -> Result<AnnounceResponse> {
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }

    let announce_message = match Announce::depacketize(payload_buf) {
        Ok(announce_message) => announce_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let announce_response =
        announce_handler(announce_message, client, track_namespace_manager_repository).await;

    match announce_response {
        Ok(announce_response_message) => match announce_response_message {
            AnnounceResponse::Success(ok_message) => {
                ok_message.packetize(write_buf);
                Ok(AnnounceResponse::Success(ok_message))
            }
            AnnounceResponse::Failure(err_message) => {
                err_message.packetize(write_buf);
                Ok(AnnounceResponse::Failure(err_message))
            }
        },
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    }
}
