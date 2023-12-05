use anyhow::Result;

use crate::{
    modules::messages::unsubscribe_message::UnsubscribeMessage, MOQTClient, TrackManagerRepository,
};

pub(crate) enum UnSubscribeResponse {
    Success,
    Failure,
}

pub(crate) async fn unsubscribe_handler(
    unsubscribe_message: UnsubscribeMessage,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackManagerRepository,
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

    // TODO: unsubscribe情報を削除

    // tmp
    Ok(UnSubscribeResponse::Success)
}
