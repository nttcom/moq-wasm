use anyhow::{bail, Result};

use crate::{
    message_handler::StreamType,
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::subscribe_ok::SubscribeOk,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    SendStreamDispatcherRepository,
};

pub(crate) async fn subscribe_ok_handler(
    subscribe_ok_message: SubscribeOk,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
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
    let subscriber_session_ids = track_namespace_manager_repository
        .get_subscriber_session_ids_by_track_namespace_and_track_name(
            subscribe_ok_message.track_namespace(),
            subscribe_ok_message.track_name(),
        )
        .await;
    match subscriber_session_ids {
        Some(session_ids) => {
            // Notify all waiting subscribers with the SUBSCRIBE_OK message
            for session_id in session_ids.iter() {
                let message: Box<dyn MOQTPayload> = Box::new(subscribe_ok_message.clone());
                tracing::info!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    subscribe_ok_message,
                    session_id
                );
                match send_stream_dispatcher_repository
                    .send_message_to_send_stream_thread(*session_id, message, StreamType::Bi)
                    .await
                {
                    Ok(_) => {
                        // Record the track_id upon success and activate the subscriber
                        let _ = track_namespace_manager_repository
                            .set_track_id(
                                subscribe_ok_message.track_namespace(),
                                subscribe_ok_message.track_name(),
                                subscribe_ok_message.track_id(),
                            )
                            .await;
                        let _ = track_namespace_manager_repository
                            .activate_subscriber(
                                subscribe_ok_message.track_namespace(),
                                subscribe_ok_message.track_name(),
                                *session_id,
                            )
                            .await;
                    }
                    Err(e) => {
                        tracing::error!(
                            "relay subscribe ok failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                        bail!("relay subscribe ok failed: {:?}", e);
                    }
                }
            }
            Ok(())
        }
        None => bail!("waiting subscriber session ids not found"),
    }
}
