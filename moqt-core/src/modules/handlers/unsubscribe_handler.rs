use anyhow::Result;

use crate::{
    modules::messages::unsubscribe_message::UnsubscribeMessage, MOQTClient, TrackManagerRepository,
};

// TODO: 未実装のためallow dead_codeをつけている
#[allow(dead_code)]
pub(crate) enum UnSubscribeResponse {
    Success,
    Failure,
}

// TODO: 未実装のため_をつけている
pub(crate) async fn _unsubscribe_handler(
    unsubscribe_message: UnsubscribeMessage,
    _client: &mut MOQTClient, // TODO: 未実装のため_をつけている
    _track_manager_repository: &mut dyn TrackManagerRepository, // TODO: 未実装のため_をつけている
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

    // FIXME: tmp
    Ok(UnSubscribeResponse::Success)
}
