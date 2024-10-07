use crate::modules::handlers::subscribe_handler::subscribe_handler;
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    messages::{
        control_messages::{subscribe::Subscribe, subscribe_ok::SubscribeOk},
        moqt_payload::MOQTPayload,
    },
    MOQTClient, SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
};

pub(crate) async fn process_subscribe_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    write_buf: &mut BytesMut,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> Result<Option<SubscribeOk>> {
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
        track_namespace_manager_repository,
        send_stream_dispatcher_repository,
    )
    .await;

    match result.as_ref() {
        Ok(subscribe_ok) => {
            if subscribe_ok.is_some() {
                // If the Track already exists, the Relay responds with a SubscribeOK message.
                subscribe_ok.as_ref().unwrap().packetize(write_buf);
            }

            result
        }
        Err(err) => {
            tracing::error!("subscribe_handler: err: {:?}", err.to_string());
            bail!(err.to_string());
        }
    }
}
