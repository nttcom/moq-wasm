use anyhow::Result;

use crate::{
    modules::{
        messages::{
            announce_error_message::AnnounceError,
            announce_message::{self, AnnounceMessage},
            announce_ok_message::AnnounceOk,
        },
        track_manager_repository::TrackManagerRepository,
    },
    MOQTClient,
};

pub(crate) enum AnnounceResponse {
    Success(AnnounceOk),
    Failure(AnnounceError),
}

pub(crate) async fn announce_handler(
    announce_message: AnnounceMessage,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackManagerRepository,
) -> Result<AnnounceResponse> {
    tracing::info!("announce_handler!");

    tracing::info!(
        "announce_handler: track_namespace: \"{}\"",
        announce_message.track_namespace()
    );

    // announceされたTrack Namespaceを記録
    let set_result = track_manager_repository
        .set(announce_message.track_namespace())
        .await;

    match set_result {
        // TODO: 接続しているクライアントに対して、announceされたことを通知する
        Ok(_) => Ok(AnnounceResponse::Success(AnnounceOk::new(
            announce_message.track_namespace().to_string(),
        ))),
        Err(err) => {
            tracing::info!("announce_handler: err: {:?}", err.to_string());

            Ok(AnnounceResponse::Failure(AnnounceError::new(
                announce_message.track_namespace().to_string(),
                1,
                String::from("already exist"),
            )))
        }
    }
}
