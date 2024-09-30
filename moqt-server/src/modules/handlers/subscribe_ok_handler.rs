use anyhow::Result;

use moqt_core::{
    messages::{moqt_payload::MOQTPayload, subscribe_ok::SubscribeOk},
    constants::StreamDirection,
    SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
};

pub(crate) async fn subscribe_ok_handler(
    subscribe_ok_message: SubscribeOk,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<()> {
    tracing::trace!("subscribe_ok_handler start.");

    tracing::debug!("subscribe_ok_message: {:#?}", subscribe_ok_message);

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
                tracing::debug!(
                    "message: {:#?} is sent to relay handler for client {:?}",
                    subscribe_ok_message,
                    session_id
                );
                match send_stream_dispatcher_repository
                    .send_message_to_send_stream_thread(*session_id, message, StreamDirection::Bi)
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

                        tracing::trace!("subscribe_ok_handler complete.");
                    }
                    Err(e) => {
                        tracing::warn!(
                            "relay subscribe ok failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                        // Failure on SUBSCRIBE_OK relay doesn't turn into closing connection
                    }
                }
            }
        }
        None => {
            tracing::warn!("waiting subscriber session ids not found");
            // Absence of SUBSCRIBE_OK relay doesn't turn into closing connection
        }
    }

    Ok(())
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::subscribe_ok_handler::subscribe_ok_handler;
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::{moqt_payload::MOQTPayload, subscribe_ok::SubscribeOk};
    use moqt_core::TrackNamespaceManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate SUBSCRIBE_OK message
        let track_namespace = "track_namespace";
        let track_name = "track_name";
        let track_id = 1;
        let expires = 2;
        let subscribe_ok = SubscribeOk::new(
            track_namespace.to_string(),
            track_name.to_string(),
            track_id,
            expires,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;
        let subscriber_session_id = 2;

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: subscriber_session_id,
                stream_direction: "bidirectional_stream".to_string(),
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::handlers::subscribe_ok_handler::subscribe_ok_handler;
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::{moqt_payload::MOQTPayload, subscribe_ok::SubscribeOk};
    use moqt_core::TrackNamespaceManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn relay_fail() {
        // Generate SUBSCRIBE_OK message
        let track_namespace = "track_namespace";
        let track_name = "track_name";
        let track_id = 1;
        let expires = 2;
        let subscribe_ok = SubscribeOk::new(
            track_namespace.to_string(),
            track_name.to_string(),
            track_id,
            expires,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;
        let subscriber_session_id = 2;

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscriber_not_found() {
        // Generate SUBSCRIBE_OK message
        let track_namespace = "track_namespace";
        let track_name = "track_name";
        let track_id = 1;
        let expires = 2;
        let subscribe_ok = SubscribeOk::new(
            track_namespace.to_string(),
            track_name.to_string(),
            track_id,
            expires,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        // Generate TrackNamespaceManager (without set subscriber)
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;
        let subscriber_session_id = 2;

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: subscriber_session_id,
                stream_direction: "bidirectional_stream".to_string(),
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_ok_handler and get result
        let result = subscribe_ok_handler(
            subscribe_ok,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }
}
