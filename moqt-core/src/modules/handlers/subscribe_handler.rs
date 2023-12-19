use anyhow::Result;

use crate::{
    modules::{
        messages::{
            announce_error_message::AnnounceError,
            announce_message::{self, AnnounceMessage},
            announce_ok_message::AnnounceOk,
            subscribe_error_message::SubscribeError,
            subscribe_ok_message::SubscribeOk,
            subscribe_request_message::SubscribeRequestMessage,
        },
        track_manager_repository::TrackManagerRepository,
    },
    MOQTClient,
};

pub(crate) enum SubscribeResponse {
    Success(SubscribeOk),
    Failure(SubscribeError),
}

pub(crate) async fn subscribe_handler(
    subscribe_message: SubscribeRequestMessage,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackManagerRepository,
) -> Result<SubscribeResponse> {
    tracing::info!("subscribe_handler!");

    tracing::info!(
        "subscribe_handler: track_namespace: \"{}\"",
        subscribe_message.track_namespace()
    );
    tracing::info!(
        "subscribe_handler: track_name: \"{}\"",
        subscribe_message.track_name()
    );

    // TODO: subscribe情報を登録

    // TODO: subscriber -> relayならrelay -> publisherに伝える

    // FIXME: tmp
    Ok(SubscribeResponse::Success(SubscribeOk::new(
        subscribe_message.track_namespace().to_string(),
        subscribe_message.track_name().to_string(),
        1, // tmp
        0, // unlimited
    )))
}
