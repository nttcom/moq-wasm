use crate::modules::moqt_client::MOQTClient;
use anyhow::Result;
use moqt_core::{
    constants::StreamDirection,
    messages::control_messages::{subscribe, unsubscribe::Unsubscribe},
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

    // UNSUBSCRIBEのロジック
    // PubSubRelationManagerからSubscriberを削除する
    pubsub_relation_manager_repository
        .delete_downstream_subscription(downstream_session_id, downstream_subscribe_id)
        .await?;
    // SUBSCRIBE_DONEを返す
    let subscribe_done_message = Box::new(SubscribeDone::new(downstream_subscribe_id));
    let _ = send_stream_dispatcher_repository
        .transfer_message_to_send_stream_thread(
            client.id(),
            subscribe_done_message,
            StreamDirection::Bi,
        )
        .await;

    // Subscriberの数を確認し、
    // もし、Subscriberが全員いなくなった場合、relayからOriginal Publisherに向けてUNSUBSCRIBEを送信する
    // 別処理だが、UNSUBSCRIBEのレスポンスとしてSUBSCRIBE_DONEを受け取る

    tracing::trace!("unsubscribe_handler complete.");
    Ok(())
}
