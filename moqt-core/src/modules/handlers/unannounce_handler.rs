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
    tracing::trace!("unannounce_handler start.");

    tracing::debug!("unannounce_message: {:#?}", unannounce_message);

    // Remove the announced Track Namespace
    let delete_result = track_namespace_manager_repository
        .delete_publisher(unannounce_message.track_namespace())
        .await;

    match delete_result {
        // TODO: Notify connected clients that unannouncing has occurred
        Ok(_) => {
            tracing::trace!("unannounce_handler complete.");
            Ok(())
        }
        Err(err) => {
            tracing::error!("unannounce_handler: err: {:?}", err.to_string());

            Ok(())
        }
    }
}
