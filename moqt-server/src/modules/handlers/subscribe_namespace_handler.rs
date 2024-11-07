use anyhow::Result;
use moqt_core::{
    constants::StreamDirection,
    messages::{
        control_messages::{
            announce::Announce, subscribe_namespace::SubscribeNamespace,
            subscribe_namespace_error::SubscribeNamespaceError,
            subscribe_namespace_ok::SubscribeNamespaceOk,
        },
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    SendStreamDispatcherRepository,
};

use crate::modules::moqt_client::MOQTClient;

pub(crate) async fn subscribe_namespace_handler(
    subscribe_namespace_message: SubscribeNamespace,
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<Option<SubscribeNamespaceError>> {
    tracing::trace!("subscribe_namespace_handler start.");
    tracing::debug!(
        "subscribe_namespace_message: {:#?}",
        subscribe_namespace_message
    );

    // TODO: auth

    let track_namespace_prefix = subscribe_namespace_message.track_namespace_prefix().clone();

    // Record the subscribed Track Namespace Prefix
    let set_result = pubsub_relation_manager_repository
        .set_downstream_subscribed_namespace_prefix(track_namespace_prefix.clone(), client.id())
        .await;

    match set_result {
        Ok(_) => {
            tracing::info!(
                "subscribe_namespaced track_namespace_prefix: {:#?}",
                track_namespace_prefix.clone()
            );
            tracing::trace!("subscribe_namespace_handler complete.");

            // Send SubscribeNamespaceOk message
            let subscribe_namespace_ok_message: Box<dyn MOQTPayload> =
                Box::new(SubscribeNamespaceOk::new(track_namespace_prefix.clone()));

            // TODO: Unify the method to send a message to the opposite client itself
            let _ = send_stream_dispatcher_repository
                .forward_message_to_send_stream_thread(
                    client.id(),
                    subscribe_namespace_ok_message,
                    StreamDirection::Bi,
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
                    subscribe_namespace_message.parameters().clone(),
                ));

                let _ = send_stream_dispatcher_repository
                    .forward_message_to_send_stream_thread(
                        client.id(),
                        announce_message,
                        StreamDirection::Bi,
                    )
                    .await;
            }

            Ok(None)
        }

        // TODO: Separate namespace prefix overlap error
        Err(err) => {
            let msg = std::format!(
                "subscribe_namespace_handler: set namespace prefix err: {:?}",
                err.to_string()
            );
            tracing::error!(msg);

            Ok(Some(SubscribeNamespaceError::new(
                track_namespace_prefix,
                1,
                msg,
            )))
        }
    }
}

#[cfg(test)]
mod success {
    use crate::modules::{
        handlers::subscribe_namespace_handler::subscribe_namespace_handler,
        moqt_client::MOQTClient,
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
            wrapper::PubSubRelationManagerWrapper,
        },
        send_stream_dispatcher::{
            send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
        },
    };
    use moqt_core::{
        constants::StreamDirection,
        messages::{
            control_messages::{
                subscribe_namespace::SubscribeNamespace,
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
        // Generate SUBSCRIBE_NAMESPACE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_namespace_message =
            SubscribeNamespace::new(track_namespace_prefix.clone(), parameters);
        let mut buf = bytes::BytesMut::new();
        subscribe_namespace_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_namespace_handler and get result
        let result = subscribe_namespace_handler(
            subscribe_namespace_message,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::{
        handlers::subscribe_namespace_handler::subscribe_namespace_handler,
        moqt_client::MOQTClient,
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
            wrapper::PubSubRelationManagerWrapper,
        },
        send_stream_dispatcher::{
            send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
        },
    };
    use moqt_core::{
        constants::StreamDirection,
        messages::{
            control_messages::{
                subscribe_namespace::SubscribeNamespace,
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
        // Generate SUBSCRIBE_NAMESPACE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_namespace_message =
            SubscribeNamespace::new(track_namespace_prefix.clone(), parameters);
        let mut buf = bytes::BytesMut::new();
        subscribe_namespace_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_namespace_handler and get result
        let result = subscribe_namespace_handler(
            subscribe_namespace_message,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        match result {
            Ok(Some(subscribe_namespace_error)) => {
                assert_eq!(
                    *subscribe_namespace_error.track_namespace_prefix(),
                    track_namespace_prefix
                );
                assert_eq!(subscribe_namespace_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }

    #[tokio::test]
    async fn prefix_overlap_longer() {
        // Generate SUBSCRIBE_NAMESPACE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);
        let exists_track_namespace_prefix = Vec::from(["aaa".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_namespace_message =
            SubscribeNamespace::new(track_namespace_prefix.clone(), parameters);
        let mut buf = bytes::BytesMut::new();
        subscribe_namespace_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_namespace_handler and get result
        let result = subscribe_namespace_handler(
            subscribe_namespace_message,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        match result {
            Ok(Some(subscribe_namespace_error)) => {
                assert_eq!(
                    *subscribe_namespace_error.track_namespace_prefix(),
                    track_namespace_prefix
                );
                assert_eq!(subscribe_namespace_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }

    #[tokio::test]
    async fn prefix_overlap_shorter() {
        // Generate SUBSCRIBE_NAMESPACE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);
        let exists_track_namespace_prefix =
            Vec::from(["aaa".to_string(), "bbb".to_string(), "ddd".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_namespace_message =
            SubscribeNamespace::new(track_namespace_prefix.clone(), parameters);
        let mut buf = bytes::BytesMut::new();
        subscribe_namespace_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_namespace_handler and get result
        let result = subscribe_namespace_handler(
            subscribe_namespace_message,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

        match result {
            Ok(Some(subscribe_namespace_error)) => {
                assert_eq!(
                    *subscribe_namespace_error.track_namespace_prefix(),
                    track_namespace_prefix
                );
                assert_eq!(subscribe_namespace_error.error_code(), 1);
            }
            _ => panic!("Unexpected result: {:?}", result),
        }
    }

    #[tokio::test]
    async fn relay_fail() {
        // Generate SUBSCRIBE_NAMESPACE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["aaa".to_string(), "bbb".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_namespace_message =
            SubscribeNamespace::new(track_namespace_prefix.clone(), parameters);
        let mut buf = bytes::BytesMut::new();
        subscribe_namespace_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Generate SendStreamDispacher (without set sender)
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute subscribe_namespace_handler and get result
        let result = subscribe_namespace_handler(
            subscribe_namespace_message,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn namespace_not_found() {
        // Generate SUBSCRIBE_NAMESPACE message
        let track_namespace = Vec::from(["aaa".to_string(), "bbb".to_string(), "ccc".to_string()]);
        let track_namespace_prefix = Vec::from(["ddd".to_string(), "eee".to_string()]);

        let parameter_value = "test".to_string();
        let parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(parameter_value));
        let parameters = vec![parameter];
        let subscribe_namespace_message =
            SubscribeNamespace::new(track_namespace_prefix.clone(), parameters);
        let mut buf = bytes::BytesMut::new();
        subscribe_namespace_message.packetize(&mut buf);

        // Generate client
        let upstream_session_id = 0;
        let downstream_session_id = 1;
        let mut client = MOQTClient::new(downstream_session_id);

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

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: upstream_session_id,
                stream_direction: StreamDirection::Bi,
                sender: uni_relay_tx,
            })
            .await;

        // Execute subscribe_namespace_handler and get result
        let result = subscribe_namespace_handler(
            subscribe_namespace_message,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }
}
