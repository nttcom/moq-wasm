use anyhow::Result;

use crate::{
    messages::moqt_payload::MOQTPayload,
    modules::{
        messages::subscribe_ok_message::SubscribeOk,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    StreamManagerRepository,
};

pub(crate) async fn subscribe_ok_handler(
    subscribe_ok_message: SubscribeOk,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    stream_manager_repository: &mut dyn StreamManagerRepository,
) -> Result<()> {
    tracing::info!("subscribe_ok_handler!");

    tracing::info!(
        "subscribe_ok_handler: track_namespace: \"{}\"",
        subscribe_ok_message.track_namespace()
    );
    tracing::info!(
        "subscribe_ok_handler: track_name: \"{}\"",
        subscribe_ok_message.track_name()
    );
    tracing::info!(
        "subscribe_ok_handler: track_id: \"{}\"",
        subscribe_ok_message.track_id()
    );
    // track_namespaceとtrack_nameを使ってSUBSCRIBEを送信したSUBSCRIBERを判断する
    let subscriber_session_ids = track_namespace_manager_repository
        .get_subscriber_session_ids_by_track_namespace_and_track_name(
            subscribe_ok_message.track_namespace(),
            subscribe_ok_message.track_name(),
        )
        .await;
    match subscriber_session_ids {
        Some(session_ids) => {
            let mut result: Result<(), anyhow::Error> = Ok(());

            // SUBSCRIBE_OKメッセージをすべてのsubscriber(waiting)に通知する
            for session_id in session_ids.iter() {
                let message: Box<dyn MOQTPayload> = Box::new(subscribe_ok_message.clone());
                tracing::info!(
                    "message: {:#?} is relayed into client {:?}",
                    subscribe_ok_message,
                    session_id
                );
                match stream_manager_repository
                    .relay_message(*session_id, message)
                    .await
                {
                    Ok(_) => {
                        // 成功したらtrack_idを記録してsubscriberをactivateする
                        let _ = track_namespace_manager_repository
                            .set_track_id(
                                subscribe_ok_message.track_namespace(),
                                subscribe_ok_message.track_name(),
                                subscribe_ok_message.track_id(),
                            )
                            .await;
                        let _ = track_namespace_manager_repository
                            .activate_subscriber(
                                subscribe_ok_message.track_namespace(),
                                subscribe_ok_message.track_name(),
                                *session_id,
                            )
                            .await;
                    }
                    Err(e) => {
                        result = Err(anyhow::anyhow!("relay subscribe ok failed: {:?}", e));
                        tracing::error!(
                            "relay subscribe ok failed at session id {:?}:  {:?}",
                            session_id,
                            e
                        );
                    }
                }
            }
            result
        }
        None => Err(anyhow::anyhow!("waiting subscriber session ids not found")),
    }
}
