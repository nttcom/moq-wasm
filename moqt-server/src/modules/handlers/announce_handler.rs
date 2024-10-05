use anyhow::Result;

use moqt_core::{
    messages::control_messages::{
        announce::Announce, announce_error::AnnounceError, announce_ok::AnnounceOk,
    },
    track_namespace_manager_repository::TrackNamespaceManagerRepository,
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
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
) -> Result<AnnounceResponse> {
    tracing::trace!("announce_handler start.");
    tracing::debug!("announce_message: {:#?}", announce_message);

    // Record the announced Track Namespace
    let set_result = track_namespace_manager_repository
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
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::moqt_payload::MOQTPayload;
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
        let stable_id = 0;
        let mut client = MOQTClient::new(stable_id as usize);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        // Execute announce_handler and get result
        let result = announce_handler(announce_message, &mut client, &mut track_namespace_manager)
            .await
            .unwrap();

        let expected_result = AnnounceResponse::Success(AnnounceOk::new(track_namespace.clone()));

        assert_eq!(result, expected_result);
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::handlers::announce_handler::{announce_handler, AnnounceResponse};
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::{
        messages::control_messages::{
            announce::Announce,
            announce_error::AnnounceError,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        },
        moqt_client::MOQTClient,
        TrackNamespaceManagerRepository,
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
        let stable_id = 0;
        let mut client = MOQTClient::new(stable_id as usize);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        // Set the duplicated publisher in advance
        let _ = track_namespace_manager
            .set_publisher_announced_namespace(
                announce_message.track_namespace().clone(),
                client.id,
            )
            .await;

        // Execute announce_handler and get result
        let result = announce_handler(announce_message, &mut client, &mut track_namespace_manager)
            .await
            .unwrap();

        let code = 1;
        let message = "already exist".to_string();
        let expected_result =
            AnnounceResponse::Failure(AnnounceError::new(track_namespace.clone(), code, message));

        assert_eq!(result, expected_result);
    }
}
