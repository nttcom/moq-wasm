use anyhow::Result;

use crate::{
    message_handler::StreamType,
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::subscribe_request_message::SubscribeRequestMessage,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    MOQTClient, RelayHandlerManagerRepository,
};

pub(crate) async fn subscribe_handler(
    subscribe_message: SubscribeRequestMessage,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    relay_handler_manager_repository: &mut dyn RelayHandlerManagerRepository,
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
    // Since only the track_namespace is recorded in ANNOUNCE, use track_namespace to determine the publisher
    let publisher_session_id = track_namespace_manager_repository
        .get_publisher_session_id_by_track_namespace(subscribe_message.track_namespace())
        .await;
    match publisher_session_id {
        Some(session_id) => {
            // Record the SUBSCRIBER who sent the SUBSCRIBE message
            match track_namespace_manager_repository
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
            // Notify the publisher about the SUBSCRIBE message
            let message: Box<dyn MOQTPayload> = Box::new(subscribe_message.clone());
            tracing::info!(
                "message: {:#?} is sent to relay handler for client {:?}",
                subscribe_message,
                session_id
            );

            match relay_handler_manager_repository
                .send_message_to_relay_handler(session_id, message, StreamType::Bi)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => Err(anyhow::anyhow!("relay subscribe failed: {:?}", e)),
            }
        }
        None => Err(anyhow::anyhow!("publisher session id not found")),
    }
}
