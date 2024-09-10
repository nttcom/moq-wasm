use anyhow::Result;

use moqt_core::{messages::unsubscribe::Unsubscribe, MOQTClient, TrackNamespaceManagerRepository};

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
    tracing::trace!("unsubscribe_handler start.");

    tracing::debug!("unsubscribe_message: {:#?}", unsubscribe_message);
    // TODO: Remove unsubscribe information

    // FIXME: tmp
    tracing::trace!("unsubscribe_handler complete.");
    Ok(UnSubscribeResponse::Success)
}
