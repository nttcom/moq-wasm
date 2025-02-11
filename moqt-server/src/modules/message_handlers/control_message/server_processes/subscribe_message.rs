use crate::modules::moqt_client::MOQTClient;
use crate::modules::{
    message_handlers::control_message::handlers::subscribe_handler::subscribe_handler,
    object_cache_storage::wrapper::ObjectCacheStorageWrapper,
};
use crate::SenderToOpenSubscription;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::control_messages::subscribe_error::SubscribeError,
    messages::{control_messages::subscribe::Subscribe, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    SendStreamDispatcherRepository,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub(crate) async fn process_subscribe_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_downstream_stream_or_datagram_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
) -> Result<Option<SubscribeError>> {
    let subscribe_request_message = match Subscribe::depacketize(payload_buf) {
        Ok(subscribe_request_message) => subscribe_request_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let result = subscribe_handler(
        subscribe_request_message,
        client,
        pubsub_relation_manager_repository,
        send_stream_dispatcher_repository,
        object_cache_storage,
        open_downstream_stream_or_datagram_txes,
    )
    .await;

    match result.as_ref() {
        Ok(Some(subscribe_error)) => {
            subscribe_error.packetize(write_buf);

            result
        }
        Ok(None) => result,
        Err(err) => {
            tracing::error!("subscribe_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
