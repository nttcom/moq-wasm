use anyhow::{bail, Result};
use bytes::BytesMut;

use moqt_core::{
    messages::{control_messages::announce_ok::AnnounceOk, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::modules::{
    message_handlers::control_message::handlers::announce_ok_handler::announce_ok_handler,
    moqt_client::MOQTClient,
};

pub(crate) async fn process_announce_ok_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
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
