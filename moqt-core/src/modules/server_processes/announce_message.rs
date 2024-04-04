use bytes::BytesMut;

use crate::{
    handlers::announce_handler::{announce_handler, AnnounceResponse},
    messages::{announce_message::AnnounceMessage, moqt_payload::MOQTPayload},
    moqt_client::MOQTClientStatus,
    MOQTClient, TrackManagerRepository,
};
use anyhow::{bail, Result};

pub enum AnnounceType {
    Ok,
    Error,
}

pub(crate) async fn process_announce_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    write_buf: &mut BytesMut,
    track_manager_repository: &mut dyn TrackManagerRepository,
) -> Result<AnnounceType> {
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }

    let announce_message = AnnounceMessage::depacketize(payload_buf);

    if let Err(err) = announce_message {
        // fix
        tracing::info!("{:#?}", err);
        bail!(err.to_string());
    }

    let announce_result =
        announce_handler(announce_message.unwrap(), client, track_manager_repository).await;

    match announce_result {
        Ok(announce_message) => match announce_message {
            AnnounceResponse::Success(ok_message) => {
                ok_message.packetize(write_buf);
                Ok(AnnounceType::Ok)
            }
            AnnounceResponse::Failure(err_message) => {
                err_message.packetize(write_buf);
                Ok(AnnounceType::Error)
            }
        },
        Err(err) => {
            // fix
            tracing::info!("{:#?}", err);
            bail!(err.to_string());
        }
    }
}
