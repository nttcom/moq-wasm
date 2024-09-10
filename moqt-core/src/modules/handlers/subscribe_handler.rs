use anyhow::{bail, Result};

use crate::{
    message_handler::StreamType,
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::subscribe::Subscribe,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    MOQTClient, SendStreamDispatcherRepository,
};

pub(crate) async fn subscribe_handler(
    subscribe_message: Subscribe,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    tracing::trace!("subscribe_handler start.");

    tracing::debug!("subscribe_message: {:#?}", subscribe_message);

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
                    bail!("cannot register subscriber: {:?}", e);
                }
            }
            // Notify the publisher about the SUBSCRIBE message
            let message: Box<dyn MOQTPayload> = Box::new(subscribe_message.clone());
            tracing::debug!(
                "message: {:#?} is sent to relay handler for client {:?}",
                subscribe_message,
                session_id
            );

            match send_stream_dispatcher_repository
                .send_message_to_send_stream_thread(session_id, message, StreamType::Bi)
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "subscribed track_namespace: {:?}",
                        subscribe_message.track_namespace(),
                    );
                    tracing::info!(
                        "subscribed track_name: {:?}",
                        subscribe_message.track_name()
                    );
                    tracing::trace!("subscribe_handler complete.");
                    Ok(())
                }
                Err(e) => bail!("relay subscribe failed: {:?}", e),
            }
        }
        None => bail!("publisher session id not found"),
    }
}
