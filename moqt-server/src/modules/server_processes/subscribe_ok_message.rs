use crate::modules::handlers::subscribe_ok_handler::subscribe_ok_handler;
use crate::modules::moqt_client::MOQTClient;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    messages::{control_messages::subscribe_ok::SubscribeOk, moqt_payload::MOQTPayload},
    SendStreamDispatcherRepository,
};

pub(crate) async fn process_subscribe_ok_message(
    payload_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
    client: &mut MOQTClient,
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
        send_stream_dispatcher_repository,
        client,
    )
    .await
}
