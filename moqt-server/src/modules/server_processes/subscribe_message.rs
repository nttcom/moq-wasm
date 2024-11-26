use crate::modules::handlers::subscribe_handler::{subscribe_handler, SubscribeResponse};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    messages::{control_messages::subscribe::Subscribe, moqt_payload::MOQTPayload},
    MOQTClient, SendStreamDispatcherRepository,
};

pub(crate) async fn process_subscribe_message(
    payload_buf: &mut BytesMut,
    client: &MOQTClient,
    write_buf: &mut BytesMut,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<Option<SubscribeResponse>> {
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
    )
    .await;

    match result.as_ref() {
        Ok(subscribe_response) => {
            if subscribe_response.is_some() {
                match subscribe_response {
                    Some(SubscribeResponse::Success(subscribe_ok)) => {
                        subscribe_ok.packetize(write_buf);
                    }
                    Some(SubscribeResponse::Failure(subscribe_error)) => {
                        subscribe_error.packetize(write_buf);
                    }
                    None => {}
                }
            }

            result
        }
        Err(err) => {
            tracing::error!("subscribe_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
