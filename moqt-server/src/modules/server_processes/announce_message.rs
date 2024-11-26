use crate::modules::handlers::announce_handler::announce_handler;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::messages::control_messages::announce_error::AnnounceError;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::send_stream_dispatcher_repository;
use moqt_core::{
    messages::{control_messages::announce::Announce, moqt_payload::MOQTPayload},
    MOQTClient,
};

pub(crate) async fn process_announce_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn send_stream_dispatcher_repository::SendStreamDispatcherRepository,
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
