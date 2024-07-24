use anyhow::Result;

use crate::{
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::{
            subscribe_error_message::SubscribeError, subscribe_ok_message::SubscribeOk,
            subscribe_request_message::SubscribeRequestMessage,
        },
        track_manager_repository::TrackManagerRepository,
    },
    MOQTClient, StreamManagerRepository,
};

// Failureの場合は未実装のため、allow dead_codeをつけている
#[allow(dead_code)]
pub(crate) enum SubscribeResponse {
    Success(SubscribeOk),
    Failure(SubscribeError), // TODO: 未実装
}

pub(crate) async fn subscribe_handler(
    subscribe_message: SubscribeRequestMessage,
    _client: &mut MOQTClient, // TODO: 未実装のため_をつけている
    track_manager_repository: &mut dyn TrackManagerRepository,
    stream_manager_repository: &mut dyn StreamManagerRepository,
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
    // ANNOUNCEではtrack_namespaceのみを記録しているので、track_namespaceを使ってpublisherを判断する
    let publisher_session_id = track_manager_repository
        .get_publisher_session_id_by_track_namespace(subscribe_message.track_namespace())
        .await;
    match publisher_session_id {
        Some(session_id) => {
            // TODO: SUBSCRIBEメッセージを送ったSUBSCRIBERを記録する
            // SUBSCRIBEメッセージをpublisherに通知する
            let message: Box<dyn MOQTPayload> = Box::new(subscribe_message.clone());
            tracing::info!(
                "message: {:#?} is relayed into client {:?}",
                subscribe_message,
                session_id
            );
            let _ = stream_manager_repository
                .relay_message(session_id, message)
                .await;
        }
        None => {
            // SUBSCRIBE_ERRORを返す
        }
    }

    // FIXME: tmp
    Ok(SubscribeResponse::Success(SubscribeOk::new(
        subscribe_message.track_namespace().to_string(),
        subscribe_message.track_name().to_string(),
        1, // tmp
        0, // unlimited
    )))
}
