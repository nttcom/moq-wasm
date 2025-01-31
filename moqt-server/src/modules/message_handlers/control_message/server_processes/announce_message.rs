use anyhow::{bail, Result};
use bytes::BytesMut;

use moqt_core::{
    messages::{
        control_messages::{announce::Announce, announce_error::AnnounceError},
        moqt_payload::MOQTPayload,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    send_stream_dispatcher_repository::SendStreamDispatcherRepository,
};

use crate::modules::{
    message_handlers::control_message::handlers::announce_handler::announce_handler,
    moqt_client::MOQTClient,
};

pub(crate) async fn process_announce_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<Option<AnnounceError>> {
    let announce_message = match Announce::depacketize(payload_buf) {
        Ok(announce_message) => announce_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let result = announce_handler(
        announce_message,
        client,
        pubsub_relation_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await;

    match result.as_ref() {
        Ok(Some(announce_error)) => {
            announce_error.packetize(write_buf);

            result
        }
        Ok(None) => result,
        Err(err) => {
            tracing::error!("announce_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
