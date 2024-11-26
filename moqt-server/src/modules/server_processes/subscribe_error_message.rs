use crate::modules::handlers::subscribe_error_handler::subscribe_error_handler;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    messages::{control_messages::subscribe_error::SubscribeError, moqt_payload::MOQTPayload},
    MOQTClient, SendStreamDispatcherRepository,
};

pub(crate) async fn process_subscribe_error_message(
    payload_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    client: &MOQTClient,
) -> Result<()> {
    let subscribe_error_message = match SubscribeError::depacketize(payload_buf) {
        Ok(subscribe_error_message) => subscribe_error_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    subscribe_error_handler(
        subscribe_error_message,
        pubsub_relation_manager_repository,
        send_stream_dispatcher_repository,
        client,
    )
    .await
}
