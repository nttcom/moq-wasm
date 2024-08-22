use anyhow::Result;

use crate::{
    message_handler::StreamType,
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::object_message::{ObjectWithLength, ObjectWithoutLength},
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    RelayHandlerManagerRepository,
};

pub(crate) async fn object_with_payload_length_handler(
    object_with_payload_length_message: ObjectWithLength,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    relay_handler_manager_repository: &mut dyn RelayHandlerManagerRepository,
) -> Result<()> {
    tracing::info!("object_with_payload_length_handler!");

    tracing::info!(
        "object_with_payload_length_handler: track_id: \"{}\"",
        object_with_payload_length_message.track_id()
    );

    // Use track_id to determine the subscriber
    let subscriber_session_ids = track_namespace_manager_repository
        .get_subscriber_session_ids_by_track_id(object_with_payload_length_message.track_id())
        .await;

    match subscriber_session_ids {
        Some(session_ids) => {
            let mut result: Result<(), anyhow::Error> = Ok(());

            // Relay the object_with_payload_length message to all active subscribers
            for session_id in session_ids.iter() {
                let message: Box<dyn MOQTPayload> =
                    Box::new(object_with_payload_length_message.clone());
                tracing::info!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    object_with_payload_length_message,
                    session_id
                );
                match relay_handler_manager_repository
                    .send_message_to_relay_handler(*session_id, message, StreamType::Uni)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        result = Err(anyhow::anyhow!("relay object message failed: {:?}", e));
                        tracing::error!(
                            "relay object message failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                    }
                }
            }
            result
        }
        None => {
            tracing::error!("active subscriber session ids not found");
            Err(anyhow::anyhow!("active subscriber session ids not found"))
        }
    }
}

pub(crate) async fn object_without_payload_length_handler(
    object_without_payload_length_message: ObjectWithoutLength,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    relay_handler_manager_repository: &mut dyn RelayHandlerManagerRepository,
) -> Result<()> {
    tracing::info!("object_without_payload_length_handler!");

    tracing::info!(
        "object_without_payload_length_handler: track_id: \"{}\"",
        object_without_payload_length_message.track_id()
    );

    // Use track_id to determine the subscriber
    let subscriber_session_ids = track_namespace_manager_repository
        .get_subscriber_session_ids_by_track_id(object_without_payload_length_message.track_id())
        .await;
    match subscriber_session_ids {
        Some(session_ids) => {
            let mut result: Result<(), anyhow::Error> = Ok(());

            // Relay the object_without_payload_length message to all active subscribers
            for session_id in session_ids.iter() {
                let message: Box<dyn MOQTPayload> =
                    Box::new(object_without_payload_length_message.clone());
                tracing::info!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    object_without_payload_length_message,
                    session_id
                );
                match relay_handler_manager_repository
                    .send_message_to_relay_handler(*session_id, message, StreamType::Uni)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        result = Err(anyhow::anyhow!("relay object message failed: {:?}", e));
                        tracing::error!(
                            "relay object message failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                    }
                }
            }
            result
        }
        None => {
            tracing::error!("active subscriber session ids not found");
            Err(anyhow::anyhow!("active subscriber session ids not found"))
        }
    }
}
