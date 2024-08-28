use anyhow::Result;

use crate::{
    modules::{
        messages::unannounce_message::UnAnnounce,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    MOQTClient,
};

pub(crate) async fn unannounce_handler(
    unannounce_message: UnAnnounce,
    _client: &mut MOQTClient, // TODO: Not implemented yet
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
) -> Result<()> {
    tracing::info!("unannounce_handler!");

    tracing::info!(
        "unannounce_handler: track_namespace: \"{}\"",
        unannounce_message.track_namespace()
    );

    // Remove the announced Track Namespace
    let delete_result = track_namespace_manager_repository
        .delete_publisher(unannounce_message.track_namespace())
        .await;

    match delete_result {
        // TODO: Notify connected clients that unannouncing has occurred
        Ok(_) => Ok(()),
        Err(err) => {
            tracing::info!("unannounce_handler: err: {:?}", err.to_string());

            Ok(())
        }
    }
}
