use crate::modules::handlers::subscribe_announces_handler::subscribe_announces_handler;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::messages::control_messages::subscribe_announces_error::SubscribeAnnouncesError;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::SendStreamDispatcherRepository;
use moqt_core::{
    messages::{
        control_messages::subscribe_announces::SubscribeAnnounces, moqt_payload::MOQTPayload,
    },
    MOQTClient,
};

pub(crate) async fn process_subscribe_announces_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
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
        send_stream_dispatcher_repository,
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
