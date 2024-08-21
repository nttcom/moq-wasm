use anyhow::Result;

use crate::{
    message_handler::StreamType,
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::subscribe_ok_message::SubscribeOk,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    RelayHandlerManagerRepository,
};

pub(crate) async fn subscribe_ok_handler(
    subscribe_ok_message: SubscribeOk,
    track_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    relay_handler_manager_repository: &mut dyn RelayHandlerManagerRepository,
) -> Result<()> {
    tracing::info!("subscribe_ok_handler!");

    tracing::info!(
        "subscribe_ok_handler: track_namespace: \"{}\"",
        subscribe_ok_message.track_namespace()
    );
    tracing::info!(
        "subscribe_ok_handler: track_name: \"{}\"",
        subscribe_ok_message.track_name()
    );
    tracing::info!(
        "subscribe_ok_handler: track_id: \"{}\"",
        subscribe_ok_message.track_id()
    );
    // Determine the SUBSCRIBER who sent the SUBSCRIBE using the track_namespace and track_name
    let subscriber_session_ids = track_manager_repository
        .get_subscriber_session_ids_by_track_namespace_and_track_name(
            subscribe_ok_message.track_namespace(),
            subscribe_ok_message.track_name(),
        )
        .await;
    match subscriber_session_ids {
        Some(session_ids) => {
            let mut result: Result<(), anyhow::Error> = Ok(());

            // Notify all waiting subscribers with the SUBSCRIBE_OK message
            for session_id in session_ids.iter() {
                let message: Box<dyn MOQTPayload> = Box::new(subscribe_ok_message.clone());
                tracing::info!(
                    "message: {:#?} is relayed into client {:?}",
                    subscribe_ok_message,
                    session_id
                );
                match relay_handler_manager_repository
                    .send_message_to_relay_handler(*session_id, message, StreamType::Bi)
                    .await
                {
                    Ok(_) => {
                        // Record the track_id upon success and activate the subscriber
                        let _ = track_manager_repository
                            .set_track_id(
                                subscribe_ok_message.track_namespace(),
                                subscribe_ok_message.track_name(),
                                subscribe_ok_message.track_id(),
                            )
                            .await;
                        let _ = track_manager_repository
                            .activate_subscriber(
                                subscribe_ok_message.track_namespace(),
                                subscribe_ok_message.track_name(),
                                *session_id,
                            )
                            .await;
                    }
                    Err(e) => {
                        result = Err(anyhow::anyhow!("relay subscribe ok failed: {:?}", e));
                        tracing::error!(
                            "relay subscribe ok failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                    }
                }
            }
            result
        }
        None => Err(anyhow::anyhow!("waiting subscriber session ids not found")),
    }
}
