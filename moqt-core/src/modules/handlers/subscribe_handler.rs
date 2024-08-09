use anyhow::Result;

use crate::{
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::subscribe_request_message::SubscribeRequestMessage,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    MOQTClient, StreamManagerRepository,
};

pub(crate) async fn subscribe_handler(
    subscribe_message: SubscribeRequestMessage,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    stream_manager_repository: &mut dyn StreamManagerRepository,
) -> Result<()> {
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
    let publisher_session_id = track_namespace_manager_repository
        .get_publisher_session_id_by_track_namespace(subscribe_message.track_namespace())
        .await;
    match publisher_session_id {
        Some(session_id) => {
            // SUBSCRIBEメッセージを送ったSUBSCRIBERを記録する
            match track_manager_repository
                .set_subscriber(
                    subscribe_message.track_namespace(),
                    client.id,
                    subscribe_message.track_name(),
                )
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    return Err(anyhow::anyhow!("cannot register subscriber: {:?}", e));
                }
            }
            // SUBSCRIBEメッセージをpublisherに通知する
            let message: Box<dyn MOQTPayload> = Box::new(subscribe_message.clone());
            tracing::info!(
                "message: {:#?} is relayed into client {:?}",
                subscribe_message,
                session_id
            );

            match stream_manager_repository
                .relay_message(session_id, message)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => Err(anyhow::anyhow!("relay subscribe failed: {:?}", e)),
            }
        }
        None => Err(anyhow::anyhow!("publisher session id not found")),
    }
}
