use anyhow::Result;

use crate::{
    modules::{
        messages::{announce::Announce, announce_error::AnnounceError, announce_ok::AnnounceOk},
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    MOQTClient,
};

pub(crate) enum AnnounceResponse {
    Success(AnnounceOk),
    Failure(AnnounceError),
}

pub(crate) async fn announce_handler(
    announce_message: Announce,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
) -> Result<AnnounceResponse> {
    tracing::trace!("announce_handler start.");
    tracing::debug!("announce_message: {:#?}", announce_message);

    // Record the announced Track Namespace
    let set_result = track_namespace_manager_repository
        .set_publisher(announce_message.track_namespace(), client.id)
        .await;

    match set_result {
        Ok(_) => {
            let track_namespace = announce_message.track_namespace().to_string();

            tracing::info!("announced track_namespace: {:#?}", track_namespace);
            tracing::trace!("announce_handler complete.");

            Ok(AnnounceResponse::Success(AnnounceOk::new(
                track_namespace.to_string(),
            )))
        }
        Err(err) => {
            tracing::error!("announce_handler: err: {:?}", err.to_string());

            Ok(AnnounceResponse::Failure(AnnounceError::new(
                announce_message.track_namespace().to_string(),
                1,
                String::from("already exist"),
            )))
        }
    }
}
