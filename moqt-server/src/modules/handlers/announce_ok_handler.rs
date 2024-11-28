use anyhow::Result;

use moqt_core::{
    messages::control_messages::announce_ok::AnnounceOk,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::modules::moqt_client::MOQTClient;

pub(crate) async fn announce_ok_handler(
    announce_ok_message: AnnounceOk,
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
) -> Result<()> {
    tracing::trace!("announce_ok_handler start.");
    tracing::debug!("announce_ok_message: {:#?}", announce_ok_message);

    pubsub_relation_manager_repository
        .set_downstream_announced_namespace(
            announce_ok_message.track_namespace().clone(),
            client.id(),
        )
        .await?;

    Ok(())
}

#[cfg(test)]
mod success {
    use crate::modules::handlers::announce_ok_handler::announce_ok_handler;
    use crate::modules::moqt_client::MOQTClient;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use moqt_core::messages::control_messages::announce_ok::AnnounceOk;
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn normal_case() {
        // Generate ANNOUNCE message
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let announce_ok_message = AnnounceOk::new(track_namespace.clone());
        let mut buf = bytes::BytesMut::new();
        announce_ok_message.packetize(&mut buf);

        // Generate client
        let downstream_session_id = 0;
        let client = MOQTClient::new(downstream_session_id);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        let max_subscribe_id = 10;

        let _ = pubsub_relation_manager
            .setup_subscriber(max_subscribe_id, downstream_session_id)
            .await;

        // Execute announce_ok_handler and get result
        let result =
            announce_ok_handler(announce_ok_message, &client, &mut pubsub_relation_manager).await;

        assert!(result.is_ok());
    }
}
