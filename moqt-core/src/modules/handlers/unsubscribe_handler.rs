use anyhow::Result;

use crate::{
    modules::messages::unsubscribe_message::Unsubscribe, MOQTClient,
    TrackNamespaceManagerRepository,
};

// TODO: Not implemented yet
#[allow(dead_code)]
pub(crate) enum UnSubscribeResponse {
    Success,
    Failure,
}

// TODO: Not implemented yet
pub(crate) async fn _unsubscribe_handler(
    unsubscribe_message: Unsubscribe,
    _client: &mut MOQTClient, // TODO: Not implemented yet
    _track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository, // TODO: Not implemented yet
) -> Result<UnSubscribeResponse> {
    tracing::info!("unsubscribe_handler!");

    tracing::info!(
        "unsubscribe_handler: track_namespace: \"{}\"",
        unsubscribe_message.track_namespace()
    );
    tracing::info!(
        "unsubscribe_handler: track_name: \"{}\"",
        unsubscribe_message.track_name()
    );

    // TODO: Remove unsubscribe information

    // FIXME: tmp
    Ok(UnSubscribeResponse::Success)
}
