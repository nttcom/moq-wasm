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
    MOQTClient, SendStreamDispatcherRepository,
};

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
        .set_downstream_subscribed_namespace_prefix(track_namespace_prefix.clone(), client.id)
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
                .send_message_to_send_stream_thread(
                    client.id,
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
                    .send_message_to_send_stream_thread(
                        client.id,
                        announce_message,
                        StreamDirection::Bi,
                    )
                    .await;
            }

            Ok(None)
        }
        // TODO: Check if “subscribe namespace overlap” should turn into closing connection
        Err(err) => {
            tracing::error!("subscribe_namespace_handler: err: {:?}", err.to_string());

            Ok(Some(SubscribeNamespaceError::new(
                track_namespace_prefix,
                1,
                String::from("subscribe namespace overlap"),
            )))
        }
    }
}
