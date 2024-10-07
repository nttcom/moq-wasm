use anyhow::Result;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    messages::control_messages::{
        announce::Announce, announce_error::AnnounceError, announce_ok::AnnounceOk,
    },
    MOQTClient,
};

#[derive(Debug, PartialEq)]
pub(crate) enum AnnounceResponse {
    Success(AnnounceOk),
    Failure(AnnounceError),
}

pub(crate) async fn announce_handler(
    announce_message: Announce,
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
) -> Result<AnnounceResponse> {
    tracing::trace!("announce_handler start.");
    tracing::debug!("announce_message: {:#?}", announce_message);

    // Record the announced Track Namespace
    let set_result = pubsub_relation_manager_repository
        .set_publisher_announced_namespace(announce_message.track_namespace().clone(), client.id)
        .await;

    match set_result {
        Ok(_) => {
            let track_namespace = announce_message.track_namespace();

            tracing::info!("announced track_namespace: {:#?}", track_namespace);
            tracing::trace!("announce_handler complete.");

            Ok(AnnounceResponse::Success(AnnounceOk::new(
                track_namespace.clone(),
            )))
        }
        // TODO: Check if “already exist” should turn into closing connection
        Err(err) => {
            tracing::error!("announce_handler: err: {:?}", err.to_string());

            Ok(AnnounceResponse::Failure(AnnounceError::new(
                announce_message.track_namespace().clone(),
                1,
                String::from("already exist"),
            )))
        }
    }
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::announce_handler::{announce_handler, AnnounceResponse};
    use crate::modules::relation_manager::{
        commands::PubSubRelationCommand, interface::PubSubRelationManagerInterface,
        manager::pubsub_relation_manager,
    };
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use moqt_core::{
        messages::control_messages::{
            announce::Announce,
            announce_ok::AnnounceOk,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        moqt_client::MOQTClient,
    };
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate ANNOUNCE message
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let number_of_parameters = 1;

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 0;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;

        // Execute announce_handler and get result
        let result = announce_handler(announce_message, &mut client, &mut pubsub_relation_manager)
            .await
            .unwrap();

        let expected_result = AnnounceResponse::Success(AnnounceOk::new(track_namespace.clone()));

        assert_eq!(result, expected_result);
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::handlers::announce_handler::{announce_handler, AnnounceResponse};
    use crate::modules::relation_manager::{
        commands::PubSubRelationCommand, interface::PubSubRelationManagerInterface,
        manager::pubsub_relation_manager,
    };
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use moqt_core::{
        messages::control_messages::{
            announce::Announce,
            announce_error::AnnounceError,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        moqt_client::MOQTClient,
    };
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn publisher_already_exists() {
        // Generate ANNOUNCE message
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let number_of_parameters = 1;

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let announce_message =
            Announce::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        // Generate client
        let publisher_session_id = 0;
        let mut client = MOQTClient::new(publisher_session_id);

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, publisher_session_id)
            .await;

        // Set the duplicated publisher in advance
        let _ = pubsub_relation_manager
            .set_publisher_announced_namespace(
                announce_message.track_namespace().clone(),
                client.id,
            )
            .await;

        // Execute announce_handler and get result
        let result = announce_handler(announce_message, &mut client, &mut pubsub_relation_manager)
            .await
            .unwrap();

        let code = 1;
        let message = "already exist".to_string();
        let expected_result =
            AnnounceResponse::Failure(AnnounceError::new(track_namespace.clone(), code, message));

        assert_eq!(result, expected_result);
    }
}
