use crate::modules::moqt_client::MOQTClient;
use anyhow::Result;
use moqt_core::{
    constants::StreamDirection,
    messages::control_messages::{
        subscribe_done::{StatusCode, SubscribeDone},
        unsubscribe::Unsubscribe,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    SendStreamDispatcherRepository,
};

pub(crate) async fn unsubscribe_handler(
    unsubscribe_message: Unsubscribe,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    client: &MOQTClient,
) -> Result<()> {
    tracing::trace!("unsubscribe_handler start.");
    tracing::debug!("unsubscribe_message: {:#?}", unsubscribe_message);

    let downstream_session_id = client.id();
    let downstream_subscribe_id = unsubscribe_message.subscribe_id();
    let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager_repository
        .get_related_publisher(downstream_session_id, downstream_subscribe_id)
        .await?;

    // 1. Delete Subscription from PubSubRelationManager
    pubsub_relation_manager_repository
        .delete_downstream_subscription(downstream_session_id, downstream_subscribe_id)
        .await?;

    // 2. Response SUBSCRIBE_DONE to Client
    let subscribe_done_message = Box::new(SubscribeDone::new(
        downstream_subscribe_id,
        StatusCode::Unsubscribed,
        "Unsubscribe from subscriber".to_string(),
        false,
        None,
        None,
    ));
    send_stream_dispatcher_repository
        .transfer_message_to_send_stream_thread(
            client.id(),
            subscribe_done_message,
            StreamDirection::Bi,
        )
        .await?;

    // 3. If the number of DownStream Subscriptions is zero, send UNSUBSCRIBE to the Original Publisher.
    let downstream_subscribers = pubsub_relation_manager_repository
        .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
        .await?;
    if downstream_subscribers.is_empty() {
        let unsubscribe_message = Box::new(Unsubscribe::new(upstream_subscribe_id));
        send_stream_dispatcher_repository
            .transfer_message_to_send_stream_thread(
                upstream_session_id,
                unsubscribe_message,
                StreamDirection::Bi,
            )
            .await?;
    }

    tracing::trace!("unsubscribe_handler complete.");
    Ok(())
}
