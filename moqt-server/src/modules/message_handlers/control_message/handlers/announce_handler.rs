use crate::modules::{
    control_message_dispatcher::ControlMessageDispatcher, moqt_client::MOQTClient,
};
use anyhow::Result;
use moqt_core::{
    messages::control_messages::{
        announce::Announce, announce_error::AnnounceError, announce_ok::AnnounceOk,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

async fn forward_announce_to_subscribers(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
    track_namespace: Vec<String>,
) -> Result<()> {
    let downstream_session_ids = match pubsub_relation_manager_repository
        .get_downstream_session_ids_by_upstream_namespace(track_namespace.clone())
        .await
    {
        Ok(downstream_session_ids) => downstream_session_ids,
        Err(err) => {
            tracing::warn!("announce_handler: err: {:?}", err.to_string());
            return Err(err);
        }
    };

    for downstream_session_id in downstream_session_ids {
        match pubsub_relation_manager_repository
            .is_namespace_announced(track_namespace.clone(), downstream_session_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                let announce_message = Box::new(Announce::new(track_namespace.clone(), vec![]));
                let _ = control_message_dispatcher
                    .transfer_message_to_control_message_sender_thread(
                        downstream_session_id,
                        announce_message,
                    )
                    .await;
            }
            Err(err) => {
                tracing::warn!("announce_handler: err: {:?}", err.to_string());
            }
        }
    }

    Ok(())
}

pub(crate) async fn announce_handler(
    announce_message: Announce,
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
) -> Result<Option<AnnounceError>> {
    tracing::trace!("announce_handler start.");
    tracing::debug!("announce_message: {:#?}", announce_message);

    let set_result = pubsub_relation_manager_repository
        .set_upstream_announced_namespace(announce_message.track_namespace().clone(), client.id())
        .await;

    match set_result {
        Ok(_) => {
            let track_namespace = announce_message.track_namespace();

            tracing::info!("announced track_namespace: {:#?}", track_namespace);

            // TODO: Unify the method to send a message to the opposite client itself
            let announce_ok_message = Box::new(AnnounceOk::new(track_namespace.clone()));
            let _ = control_message_dispatcher
                .transfer_message_to_control_message_sender_thread(client.id(), announce_ok_message)
                .await;

            // If subscribers already sent SUBSCRIBE_ANNOUNCES, send ANNOUNCE message to them
            match forward_announce_to_subscribers(
                pubsub_relation_manager_repository,
                control_message_dispatcher,
                track_namespace.clone(),
            )
            .await
            {
                Ok(_) => Ok(None),
                Err(err) => Err(err),
            }
        }
        // TODO: Allow namespace overlap
        Err(err) => {
            let msg = std::format!("announce_handler: set namespace err: {:?}", err.to_string());
            tracing::error!(msg);

            Ok(Some(AnnounceError::new(
                announce_message.track_namespace().clone(),
                1,
                msg,
            )))
        }
    }
}

#[cfg(test)]
mod success {
    use super::announce_handler;
    use crate::modules::control_message_dispatcher::{
        ControlMessageDispatchCommand, ControlMessageDispatcher, control_message_dispatcher,
    };
    use crate::modules::moqt_client::MOQTClient;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::server_processes::senders;
    use bytes::BytesMut;
    use moqt_core::messages::control_messages::{
        announce::Announce,
        version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
    };
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn announce_propagate_to_subscriber() {
        // Generate ANNOUNCE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let announce_message = Announce::new(track_namespace.clone(), parameters);
        let mut buf = BytesMut::new();
        announce_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;

        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(upstream_session_id, senders_mock);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_downstream_subscribed_namespace_prefix(
                track_namespace_prefix,
                downstream_session_id,
            )
            .await;

        // Generate ControlMessageDispacher
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);

        tokio::spawn(
            async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
        );
        let mut control_message_dispatcher: ControlMessageDispatcher =
            ControlMessageDispatcher::new(control_message_dispatch_tx.clone());

        let (message_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = control_message_dispatch_tx
            .send(ControlMessageDispatchCommand::Set {
                session_id: upstream_session_id,
                sender: message_tx.clone(),
            })
            .await;
        let _ = control_message_dispatch_tx
            .send(ControlMessageDispatchCommand::Set {
                session_id: downstream_session_id,
                sender: message_tx,
            })
            .await;

        // Execute announce_handler and get result
        let result = announce_handler(
            announce_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn already_announced_to_subscriber() {
        // Generate ANNOUNCE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let announce_message = Announce::new(track_namespace.clone(), parameters);
        let mut buf = BytesMut::new();
        announce_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(upstream_session_id, senders_mock);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_downstream_subscribed_namespace_prefix(
                track_namespace_prefix,
                downstream_session_id,
            )
            .await;

        let _ = pubsub_relation_manager
            .set_downstream_announced_namespace(track_namespace.clone(), downstream_session_id)
            .await;

        // Generate ControlMessageDispacher
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);

        tokio::spawn(
            async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
        );
        let mut control_message_dispatcher: ControlMessageDispatcher =
            ControlMessageDispatcher::new(control_message_dispatch_tx.clone());

        let (message_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = control_message_dispatch_tx
            .send(ControlMessageDispatchCommand::Set {
                session_id: upstream_session_id,
                sender: message_tx,
            })
            .await;

        // Execute announce_handler and get result
        let result = announce_handler(
            announce_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }
}

#[cfg(test)]
mod failure {
    use super::announce_handler;
    use crate::modules::control_message_dispatcher::{
        ControlMessageDispatchCommand, ControlMessageDispatcher, control_message_dispatcher,
    };
    use crate::modules::moqt_client::MOQTClient;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::server_processes::senders;
    use bytes::BytesMut;
    use moqt_core::messages::control_messages::{
        announce::Announce,
        version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
    };
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn publisher_already_exists() {
        // Generate ANNOUNCE message
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let announce_message = Announce::new(track_namespace.clone(), parameters);
        let mut buf = BytesMut::new();
        announce_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;

        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(upstream_session_id, senders_mock);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_publisher(max_subscribe_id, upstream_session_id)
            .await;

        // Set the duplicated publisher in advance
        let _ = pubsub_relation_manager
            .set_upstream_announced_namespace(
                announce_message.track_namespace().clone(),
                client.id(),
            )
            .await;

        // Generate ControlMessageDispacher
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);

        tokio::spawn(
            async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
        );
        let mut control_message_dispatcher: ControlMessageDispatcher =
            ControlMessageDispatcher::new(control_message_dispatch_tx.clone());

        let (message_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = control_message_dispatch_tx
            .send(ControlMessageDispatchCommand::Set {
                session_id: upstream_session_id,
                sender: message_tx,
            })
            .await;

        // Execute announce_handler and get result
        let result = announce_handler(
            announce_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await;

        match result {
            Ok(Some(announce_error)) => {
                assert_eq!(*announce_error.track_namespace(), track_namespace);
                assert_eq!(announce_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }
}
