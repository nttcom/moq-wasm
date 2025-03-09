use crate::modules::{
    control_message_dispatcher::ControlMessageDispatcher, moqt_client::MOQTClient,
};
use anyhow::Result;
use moqt_core::{
    messages::{
        control_messages::{
            announce::Announce, subscribe_announces::SubscribeAnnounces,
            subscribe_announces_error::SubscribeAnnouncesError,
            subscribe_announces_ok::SubscribeAnnouncesOk,
        },
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

pub(crate) async fn subscribe_announces_handler(
    subscribe_announces_message: SubscribeAnnounces,
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
) -> Result<Option<SubscribeAnnouncesError>> {
    tracing::trace!("subscribe_announces_handler start.");
    tracing::debug!(
        "subscribe_announces_message: {:#?}",
        subscribe_announces_message
    );

    // TODO: auth

    let track_namespace_prefix = subscribe_announces_message.track_namespace_prefix().clone();

    // Record the subscribed Track Namespace Prefix
    let set_result = pubsub_relation_manager_repository
        .set_downstream_subscribed_namespace_prefix(track_namespace_prefix.clone(), client.id())
        .await;

    match set_result {
        Ok(_) => {
            tracing::info!(
                "subscribe_announcesd track_namespace_prefix: {:#?}",
                track_namespace_prefix.clone()
            );
            tracing::trace!("subscribe_announces_handler complete.");

            // Send SubscribeAnnouncesOk message
            let subscribe_announces_ok_message: Box<dyn MOQTPayload> =
                Box::new(SubscribeAnnouncesOk::new(track_namespace_prefix.clone()));

            // TODO: Unify the method to send a message to the opposite client itself
            let _ = control_message_dispatcher
                .transfer_message_to_control_message_sender_thread(
                    client.id(),
                    subscribe_announces_ok_message,
                )
                .await;

            // Check if namespaces that the prefix matches exist
            let namespaces = pubsub_relation_manager_repository
                .get_upstream_namespaces_matches_prefix(track_namespace_prefix)
                .await
                .unwrap();

            for namespace in namespaces {
                // Send Announce messages
                // TODO: auth parameter
                let announce_message: Box<dyn MOQTPayload> = Box::new(Announce::new(
                    namespace,
                    subscribe_announces_message.parameters().clone(),
                ));

                let _ = control_message_dispatcher
                    .transfer_message_to_control_message_sender_thread(
                        client.id(),
                        announce_message,
                    )
                    .await;
            }

            Ok(None)
        }

        // TODO: Separate namespace prefix overlap error
        Err(err) => {
            let msg = std::format!(
                "subscribe_announces_handler: set namespace prefix err: {:?}",
                err.to_string()
            );
            tracing::error!(msg);

            Ok(Some(SubscribeAnnouncesError::new(
                track_namespace_prefix,
                1,
                msg,
            )))
        }
    }
}

#[cfg(test)]
mod success {
    use super::subscribe_announces_handler;
    use crate::modules::{
        control_message_dispatcher::{
            control_message_dispatcher, ControlMessageDispatchCommand, ControlMessageDispatcher,
        },
        moqt_client::MOQTClient,
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
            wrapper::PubSubRelationManagerWrapper,
        },
        server_processes::senders,
    };
    use bytes::BytesMut;
    use moqt_core::{
        messages::{
            control_messages::{
                subscribe_announces::SubscribeAnnounces,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_payload::MOQTPayload,
        },
        pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate SUBSCRIBE_ANNOUNCES message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_announces_message =
            SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
        let mut buf = BytesMut::new();
        subscribe_announces_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(downstream_session_id, senders_mock);

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
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
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

        // Execute subscribe_announces_handler and get result
        let result = subscribe_announces_handler(
            subscribe_announces_message,
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
    use super::subscribe_announces_handler;
    use crate::modules::{
        control_message_dispatcher::{
            control_message_dispatcher, ControlMessageDispatchCommand, ControlMessageDispatcher,
        },
        moqt_client::MOQTClient,
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
            wrapper::PubSubRelationManagerWrapper,
        },
        server_processes::senders,
    };
    use bytes::BytesMut;
    use moqt_core::{
        messages::{
            control_messages::{
                subscribe_announces::SubscribeAnnounces,
                version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
            },
            moqt_payload::MOQTPayload,
        },
        pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn same_prefix() {
        // Generate SUBSCRIBE_ANNOUNCES message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_announces_message =
            SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
        let mut buf = BytesMut::new();
        subscribe_announces_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(downstream_session_id, senders_mock);

        // Generate PubSubRelationManagerWrapper (register track_namespace_prefix in advance)
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
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_downstream_subscribed_namespace_prefix(track_namespace_prefix.clone(), client.id())
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

        // Execute subscribe_announces_handler and get result
        let result = subscribe_announces_handler(
            subscribe_announces_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await;

        match result {
            Ok(Some(subscribe_announces_error)) => {
                assert_eq!(
                    *subscribe_announces_error.track_namespace_prefix(),
                    track_namespace_prefix
                );
                assert_eq!(subscribe_announces_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }

    #[tokio::test]
    async fn prefix_overlap_longer() {
        // Generate SUBSCRIBE_ANNOUNCES message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);
        let exists_track_namespace_prefix = Vec::from(["aaa".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_announces_message =
            SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
        let mut buf = BytesMut::new();
        subscribe_announces_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(downstream_session_id, senders_mock);

        // Generate PubSubRelationManagerWrapper (register track_namespace_prefix that has same prefix in advance)
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
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_downstream_subscribed_namespace_prefix(
                exists_track_namespace_prefix.clone(),
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

        // Execute subscribe_announces_handler and get result
        let result = subscribe_announces_handler(
            subscribe_announces_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await;

        match result {
            Ok(Some(subscribe_announces_error)) => {
                assert_eq!(
                    *subscribe_announces_error.track_namespace_prefix(),
                    track_namespace_prefix
                );
                assert_eq!(subscribe_announces_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }

    #[tokio::test]
    async fn prefix_overlap_shorter() {
        // Generate SUBSCRIBE_ANNOUNCES message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);
        let exists_track_namespace_prefix =
            Vec::from(["aaa".to_string(), "bbb".to_string(), "ddd".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_announces_message =
            SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
        let mut buf = BytesMut::new();
        subscribe_announces_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(downstream_session_id, senders_mock);

        // Generate PubSubRelationManagerWrapper (register track_namespace_prefix that has same prefix in advance)
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
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        let _ = pubsub_relation_manager
            .set_downstream_subscribed_namespace_prefix(
                exists_track_namespace_prefix.clone(),
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

        // Execute subscribe_announces_handler and get result
        let result = subscribe_announces_handler(
            subscribe_announces_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await;

        match result {
            Ok(Some(subscribe_announces_error)) => {
                assert_eq!(
                    *subscribe_announces_error.track_namespace_prefix(),
                    track_namespace_prefix
                );
                assert_eq!(subscribe_announces_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }

    #[tokio::test]
    async fn forward_fail() {
        // Generate SUBSCRIBE_ANNOUNCES message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_announces_message =
            SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
        let mut buf = BytesMut::new();
        subscribe_announces_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(downstream_session_id, senders_mock);

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
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Generate ControlMessageDispacher (without set sender)
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);

        tokio::spawn(
            async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
        );
        let mut control_message_dispatcher: ControlMessageDispatcher =
            ControlMessageDispatcher::new(control_message_dispatch_tx.clone());

        // Execute subscribe_announces_handler and get result
        let result = subscribe_announces_handler(
            subscribe_announces_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn namespace_not_found() {
        // Generate SUBSCRIBE_ANNOUNCES message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["ddd".to_string(), "eee".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_announces_message =
            SubscribeAnnounces::new(track_namespace_prefix.clone(), parameters);
        let mut buf = BytesMut::new();
        subscribe_announces_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let client = MOQTClient::new(downstream_session_id, senders_mock);

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
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
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

        // Execute subscribe_announces_handler and get result
        let result = subscribe_announces_handler(
            subscribe_announces_message,
            &client,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }
}
