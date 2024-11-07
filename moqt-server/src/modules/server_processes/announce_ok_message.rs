use crate::modules::handlers::announce_ok_handler::announce_ok_handler;
use crate::modules::moqt_client::MOQTClient;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::messages::{control_messages::announce_ok::AnnounceOk, moqt_payload::MOQTPayload};
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;

pub(crate) async fn process_announce_ok_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
) -> Result<()> {
    let announce_ok_message = match AnnounceOk::depacketize(payload_buf) {
        Ok(announce_ok_message) => announce_ok_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    announce_ok_handler(
        announce_ok_message,
        client,
        pubsub_relation_manager_repository,
    )
    .await
}
