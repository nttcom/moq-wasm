use anyhow::{bail, Result};

use moqt_core::{
    constants::StreamDirection,
    messages::{
        moqt_payload::MOQTPayload,
        object::{ObjectWithPayloadLength, ObjectWithoutPayloadLength},
    },
    track_namespace_manager_repository::TrackNamespaceManagerRepository,
    SendStreamDispatcherRepository,
};

#[allow(dead_code)]
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
                    .send_message_to_send_stream_thread(*session_id, message, StreamDirection::Uni)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(
                            "relay object message failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                        bail!("relay object message failed: {:?}", e);
                    }
                }
            }

            tracing::trace!("object_with_payload_length_handler complete.");
            Ok(())
        }
        None => {
            tracing::error!("active subscriber session ids not found");
            bail!("active subscriber session ids not found");
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
                    .send_message_to_send_stream_thread(*session_id, message, StreamDirection::Uni)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(
                            "relay object message failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                        bail!("relay object message failed: {:?}", e);
                    }
                }
            }

            tracing::trace!("object_without_payload_length_handler complete.");
            Ok(())
        }
        None => {
            tracing::error!("active subscriber session ids not found");
            bail!("active subscriber session ids not found");
        }
    }
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::object_handler::{
        object_with_payload_length_handler, object_without_payload_length_handler,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::constants::StreamDirection;
    use moqt_core::messages::{
        moqt_payload::MOQTPayload,
        object::{ObjectWithPayloadLength, ObjectWithoutPayloadLength},
    };
    use moqt_core::TrackNamespaceManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn with_payload_length_normal() {
        // Generate OBJECT with payload length message
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_with_payload_length = ObjectWithPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace, track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_id)
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
                stream_direction: StreamDirection::Uni,
                sender: uni_relay_tx,
            })
            .await;

        // Execute object_with_payload_length_handler and get result
        let result = object_with_payload_length_handler(
            object_with_payload_length,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn without_payload_length_normal() {
        // Generate OBJECT with payload length message
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_without_payload_length = ObjectWithoutPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace, track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_id)
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
                stream_direction: StreamDirection::Uni,
                sender: uni_relay_tx,
            })
            .await;

        // Execute object_withouot_payload_length_handler and get result
        let result = object_without_payload_length_handler(
            object_without_payload_length,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::handlers::object_handler::{
        object_with_payload_length_handler, object_without_payload_length_handler,
    };
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::constants::StreamDirection;
    use moqt_core::messages::{
        moqt_payload::MOQTPayload,
        object::{ObjectWithPayloadLength, ObjectWithoutPayloadLength},
    };
    use moqt_core::TrackNamespaceManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn with_payload_length_subscriber_not_found() {
        // Generate OBJECT with payload length message
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_with_payload_length = ObjectWithPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        // Generate TrackNamespaceManager (without set subscriber)
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let track_namespace = "test_namespace";
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
                stream_direction: StreamDirection::Uni,
                sender: uni_relay_tx,
            })
            .await;

        // Execute object_with_payload_length_handler and get result
        let result = object_with_payload_length_handler(
            object_with_payload_length,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn without_payload_length_subscriber_not_found() {
        // Generate OBJECT with payload length message
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_without_payload_length = ObjectWithoutPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        // Generate TrackNamespaceManager (without set subscriber)
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let track_namespace = "test_namespace";
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
                stream_direction: StreamDirection::Uni,
                sender: uni_relay_tx,
            })
            .await;

        // Execute object_without_payload_length_handler and get result
        let result = object_without_payload_length_handler(
            object_without_payload_length,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn with_payload_length_relay_fail() {
        // Generate OBJECT with payload length message
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_with_payload_length = ObjectWithPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace, track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_id)
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute object_with_payload_length_handler and get result
        let result = object_with_payload_length_handler(
            object_with_payload_length,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn without_payload_length_relay_fail() {
        // Generate OBJECT with payload length message
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_without_payload_length = ObjectWithoutPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace, track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_id)
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute object_without_payload_length_handler and get result
        let result = object_without_payload_length_handler(
            object_without_payload_length,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_err());
    }
}
