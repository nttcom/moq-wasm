use anyhow::Result;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{messages::control_messages::unannounce::UnAnnounce, MOQTClient};

pub(crate) async fn unannounce_handler(
    unannounce_message: UnAnnounce,
    _client: &MOQTClient, // TODO: Not implemented yet
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
) -> Result<()> {
    tracing::trace!("unannounce_handler start.");

    tracing::debug!("unannounce_message: {:#?}", unannounce_message);

    // Remove the announced Track Namespace
    let delete_result = pubsub_relation_manager_repository
        .delete_upstream_announced_namespace(
            unannounce_message.track_namespace().clone(),
            _client.id,
        )
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
