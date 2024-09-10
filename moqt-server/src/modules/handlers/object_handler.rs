use anyhow::Result;

use moqt_core::{
    messages::{
        moqt_payload::MOQTPayload,
        object::{ObjectWithPayloadLength, ObjectWithoutPayloadLength},
    },
    stream_type::StreamType,
    track_namespace_manager_repository::TrackNamespaceManagerRepository,
    SendStreamDispatcherRepository,
};

pub(crate) async fn object_with_payload_length_handler(
    object_with_payload_length_message: ObjectWithPayloadLength,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    tracing::trace!("object_with_payload_length_handler start.");

    tracing::debug!(
        "object_with_payload_length_message: {:#?}",
        object_with_payload_length_message
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
                tracing::debug!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    object_with_payload_length_message,
                    session_id
                );
                match send_stream_dispatcher_repository
                    .send_message_to_send_stream_thread(*session_id, message, StreamType::Uni)
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
            tracing::trace!("object_with_payload_length_handler complete.");
            result
        }
        None => {
            tracing::error!("active subscriber session ids not found");
            Err(anyhow::anyhow!("active subscriber session ids not found"))
        }
    }
}

pub(crate) async fn object_without_payload_length_handler(
    object_without_payload_length_message: ObjectWithoutPayloadLength,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    tracing::trace!("object_without_payload_length_handler start.");

    tracing::debug!(
        "object_without_payload_length_message: {:#?}",
        object_without_payload_length_message
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
                tracing::debug!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    object_without_payload_length_message,
                    session_id
                );
                match send_stream_dispatcher_repository
                    .send_message_to_send_stream_thread(*session_id, message, StreamType::Uni)
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
            tracing::trace!("object_without_payload_length_handler complete.");
            result
        }
        None => {
            tracing::error!("active subscriber session ids not found");
            Err(anyhow::anyhow!("active subscriber session ids not found"))
        }
    }
}
