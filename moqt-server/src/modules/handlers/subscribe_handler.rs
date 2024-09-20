use anyhow::{bail, Result};

use moqt_core::{
    messages::{moqt_payload::MOQTPayload, subscribe::Subscribe},
    stream_type::StreamType,
    MOQTClient, SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
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
                }
                Err(e) => {
                    tracing::warn!("relay subscribe failed: {:?}", e);
                    // Failure on SUBSCRIBE_OK relay doesn't turn into closing connection
                }
            }

            Ok(())
        }

        // TODO: Check if “publisher not found” should turn into closing connection
        None => bail!("publisher session id not found"),
    }
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::subscribe_handler::subscribe_handler;
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::{
        moqt_payload::MOQTPayload,
        subscribe::{Location, Subscribe},
        version_specific_parameters::{GroupSequence, VersionSpecificParameter},
    };
    use moqt_core::MOQTClient;
    use moqt_core::TrackNamespaceManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate SUBSCRIBE message
        let track_namespace = "track_namespace";
        let track_name = "track_name".to_string();
        let start_group = Location::None;
        let start_object = Location::None;
        let end_group = Location::None;
        let end_object = Location::None;
        let version_specific_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(0));
        let track_request_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            track_namespace.to_string(),
            track_name,
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        );

        // Generate client
        let subscriber_sessin_id = 0;
        let mut client = MOQTClient::new(subscriber_sessin_id);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;
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
                session_id: publisher_session_id,
                stream_type: "bidirectional_stream".to_string(),
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::handlers::subscribe_handler::subscribe_handler;
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::{
        moqt_payload::MOQTPayload,
        subscribe::{Location, Subscribe},
        version_specific_parameters::{GroupSequence, VersionSpecificParameter},
    };
    use moqt_core::MOQTClient;
    use moqt_core::TrackNamespaceManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn cannot_register() {
        // Generate SUBSCRIBE message
        let track_namespace = "track_namespace";
        let track_name = "track_name";
        let start_group = Location::None;
        let start_object = Location::None;
        let end_group = Location::None;
        let end_object = Location::None;
        let version_specific_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(0));
        let track_request_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            track_namespace.to_string(),
            track_name.to_string(),
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        );

        // Generate client
        let subscriber_session_id = 0;
        let mut client = MOQTClient::new(subscriber_session_id);

        // Generate TrackNamespaceManager (register subscriber in advance)
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;
        let track_id = 0;

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
                session_id: publisher_session_id,
                stream_type: "bidirectional_stream".to_string(),
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn relay_fail() {
        // Generate SUBSCRIBE message
        let track_namespace = "track_namespace";
        let track_name = "track_name";
        let start_group = Location::None;
        let start_object = Location::None;
        let end_group = Location::None;
        let end_object = Location::None;
        let version_specific_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(0));
        let track_request_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            track_namespace.to_string(),
            track_name.to_string(),
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        );

        // Generate client
        let subscriber_session_id = 0;
        let mut client = MOQTClient::new(subscriber_session_id);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn publisher_not_found() {
        // Generate SUBSCRIBE message
        let track_namespace = "track_namespace";
        let track_name = "track_name";
        let start_group = Location::None;
        let start_object = Location::None;
        let end_group = Location::None;
        let end_object = Location::None;
        let version_specific_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(0));
        let track_request_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            track_namespace.to_string(),
            track_name.to_string(),
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        );

        // Generate client
        let subscriber_session_id = 0;
        let mut client = MOQTClient::new(subscriber_session_id);

        // Generate TrackNamespaceManager (without set publisher)
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        let publisher_session_id = 1;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: publisher_session_id,
                stream_type: "bidirectional_stream".to_string(),
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_handler and get result
        let result = subscribe_handler(
            subscribe,
            &mut client,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        assert!(result.is_err());
    }
}
