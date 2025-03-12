use anyhow::{bail, Result};
use bytes::BytesMut;

use moqt_core::{
    messages::{control_messages::subscribe_ok::SubscribeOk, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::modules::{
    control_message_dispatcher::ControlMessageDispatcher,
    message_handlers::control_message::handlers::subscribe_ok_handler::subscribe_ok_handler,
    moqt_client::MOQTClient,
};

pub(crate) async fn process_subscribe_ok_message(
    payload_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
    client: &MOQTClient,
) -> Result<()> {
    let subscribe_ok_message = match SubscribeOk::depacketize(payload_buf) {
        Ok(subscribe_ok_message) => subscribe_ok_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    subscribe_ok_handler(
        subscribe_ok_message,
        pubsub_relation_manager_repository,
        control_message_dispatcher,
        client,
    )
    .await
}
