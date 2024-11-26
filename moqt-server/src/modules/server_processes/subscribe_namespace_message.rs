use crate::modules::handlers::subscribe_namespace_handler::subscribe_namespace_handler;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::messages::control_messages::subscribe_namespace_error::SubscribeNamespaceError;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::SendStreamDispatcherRepository;
use moqt_core::{
    messages::{
        control_messages::subscribe_namespace::SubscribeNamespace, moqt_payload::MOQTPayload,
    },
    MOQTClient,
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
