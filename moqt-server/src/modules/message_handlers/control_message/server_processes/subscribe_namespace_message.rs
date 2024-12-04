use anyhow::{bail, Result};
use bytes::BytesMut;

use moqt_core::{
    messages::{
        control_messages::{
            subscribe_namespace::SubscribeNamespace,
            subscribe_namespace_error::SubscribeNamespaceError,
        },
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    SendStreamDispatcherRepository,
};

use crate::modules::{
    message_handlers::control_message::handlers::subscribe_namespace_handler::subscribe_namespace_handler,
    moqt_client::MOQTClient,
};

pub(crate) async fn process_subscribe_namespace_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<Option<SubscribeNamespaceError>> {
    let subscribe_namespace_message = match SubscribeNamespace::depacketize(payload_buf) {
        Ok(subscribe_namespace_message) => subscribe_namespace_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let result = subscribe_namespace_handler(
        subscribe_namespace_message,
        client,
        pubsub_relation_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await;

    match result.as_ref() {
        Ok(Some(subscribe_namespace_error)) => {
            subscribe_namespace_error.packetize(write_buf);

            result
        }
        Ok(None) => result,
        Err(err) => {
            tracing::error!("subscribe_namespace_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
