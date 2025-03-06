use crate::modules::{
    control_message_dispatcher::ControlMessageDispatcher,
    message_handlers::control_message::handlers::unsubscribe_handler::unsubscribe_handler,
    moqt_client::MOQTClient,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::{control_messages::unsubscribe::Unsubscribe, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

pub(crate) async fn process_unsubscribe_message(
    payload_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher_repository: &mut ControlMessageDispatcher,
    client: &MOQTClient,
) -> Result<()> {
    let unsubscribe_message = match Unsubscribe::depacketize(payload_buf) {
        Ok(unsubscribe_message) => unsubscribe_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    unsubscribe_handler(
        unsubscribe_message,
        pubsub_relation_manager_repository,
        control_message_dispatcher_repository,
        client,
    )
    .await
}
