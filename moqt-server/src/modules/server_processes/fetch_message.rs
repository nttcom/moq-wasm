use crate::modules::{
    handlers::fetch_handler::fetch_handler, object_cache_storage::ObjectCacheStorageWrapper,
};
use crate::SenderToOpenSubscription;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::control_messages::fetch_error::FetchError,
    messages::{control_messages::fetch::Fetch, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    MOQTClient, SendStreamDispatcherRepository,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub(crate) async fn process_fetch_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
) -> Result<Option<FetchError>> {
    let fetch_request_message = match Fetch::depacketize(payload_buf) {
        Ok(fetch_request_message) => fetch_request_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let result = fetch_handler(
        fetch_request_message,
        client,
        pubsub_relation_manager_repository,
        send_stream_dispatcher_repository,
        object_cache_storage,
        open_subscription_txes,
    )
    .await;

    match result.as_ref() {
        Ok(Some(fetch_error)) => {
            fetch_error.packetize(write_buf);

            result
        }
        Ok(None) => result,
        Err(err) => {
            tracing::error!("fetch_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
