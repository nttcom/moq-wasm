use anyhow::Result;
use moqt_core::messages::control_messages::unsubscribe::Unsubscribe;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;

use crate::modules::moqt_client::MOQTClient;

// TODO: Not implemented yet
#[allow(dead_code)]
pub(crate) enum UnSubscribeResponse {
    Success,
    Failure,
}

// TODO: Not implemented yet
// TODO: Define the behavior if the last subscriber unsubscribes from the track
pub(crate) async fn _unsubscribe_handler(
    unsubscribe_message: Unsubscribe,
    _client: &mut MOQTClient, // TODO: Not implemented yet
    _pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository, // TODO: Not implemented yet
) -> Result<UnSubscribeResponse> {
    tracing::trace!("unsubscribe_handler start.");

    tracing::debug!("unsubscribe_message: {:#?}", unsubscribe_message);
    // TODO: Remove unsubscribe information

    // FIXME: tmp
    tracing::trace!("unsubscribe_handler complete.");
    Ok(UnSubscribeResponse::Success)
}
