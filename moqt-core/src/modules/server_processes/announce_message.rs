use bytes::BytesMut;

use crate::{
    handlers::announce_handler::{announce_handler, AnnounceResponse},
    messages::{announce_message::AnnounceMessage, moqt_payload::MOQTPayload},
    moqt_client::MOQTClientStatus,
    MOQTClient, TrackManagerRepository,
};
use anyhow::{bail, Result};

pub(crate) async fn process_announce_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    write_buf: &mut BytesMut,
    track_manager_repository: &mut dyn TrackManagerRepository,
) -> Result<AnnounceResponse> {
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }

    let announce_message = match AnnounceMessage::depacketize(payload_buf) {
        Ok(announce_message) => announce_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let announce_response =
        announce_handler(announce_message, client, track_manager_repository).await;

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
            // fix
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    }
}
