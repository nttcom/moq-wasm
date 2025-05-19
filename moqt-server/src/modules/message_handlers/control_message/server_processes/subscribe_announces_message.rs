use anyhow::{Result, bail};
use bytes::BytesMut;

use moqt_core::{
    messages::{
        control_messages::{
            subscribe_announces::SubscribeAnnounces,
            subscribe_announces_error::SubscribeAnnouncesError,
        },
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::modules::{
    control_message_dispatcher::ControlMessageDispatcher,
    message_handlers::control_message::handlers::subscribe_announces_handler::subscribe_announces_handler,
    moqt_client::MOQTClient,
};

pub(crate) async fn process_subscribe_announces_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
) -> Result<Option<SubscribeAnnouncesError>> {
    let subscribe_announces_message = match SubscribeAnnounces::depacketize(payload_buf) {
        Ok(subscribe_announces_message) => subscribe_announces_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let result = subscribe_announces_handler(
        subscribe_announces_message,
        client,
        pubsub_relation_manager_repository,
        control_message_dispatcher,
    )
    .await;

    match result.as_ref() {
        Ok(Some(subscribe_announces_error)) => {
            subscribe_announces_error.packetize(write_buf);

            result
        }
        Ok(None) => result,
        Err(err) => {
            tracing::error!("subscribe_announces_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
