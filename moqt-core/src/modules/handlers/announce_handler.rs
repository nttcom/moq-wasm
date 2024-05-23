use anyhow::Result;

use crate::{
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::{
            announce_error_message::AnnounceError, announce_message::AnnounceMessage,
            announce_ok_message::AnnounceOk,
        },
        track_manager_repository::TrackManagerRepository,
    },
    stream_manager_repository::StreamManagerRepository,
    MOQTClient,
};

pub(crate) enum AnnounceResponse {
    Success(AnnounceOk),
    Failure(AnnounceError),
}

pub(crate) async fn announce_handler(
    announce_message: AnnounceMessage,
    client: &mut MOQTClient, // TODO: 未実装のため_をつけている
    track_manager_repository: &mut dyn TrackManagerRepository,
    stream_manager_repository: &mut dyn StreamManagerRepository,
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
        Ok(_) => {
            let track_namespace = announce_message.track_namespace().to_string();
            let message: Box<dyn MOQTPayload> = Box::new(announce_message.clone());
            stream_manager_repository
                .broadcast_message(Some(client.id), message)
                .await?;
            Ok(AnnounceResponse::Success(AnnounceOk::new(
                track_namespace.to_string(),
            )))
        }
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
