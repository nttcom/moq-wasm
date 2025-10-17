use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::moqt::{
        enums::{ResponseMessage, SessionEvent},
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                enums::FilterType, publish::Publish, publish_ok::PublishOk,
                request_error::RequestError,
            },
            moqt_message::MOQTMessage,
        },
        receive_message_sequence_handlers::sequence_handler_util,
        sessions::inner_session::InnerSession,
        utils,
    },
};

pub(crate) struct PublishHandler;

impl PublishHandler {
    pub(crate) async fn publish<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        tracing::info!("Publish namespace");
        let result = Publish::depacketize(&mut bytes_mut);
        if let Err(e) = result.as_ref() {
            tracing::warn!("Error has detected.");
            let err = RequestError {
                // TODO: assign correct request id.
                request_id: 0,
                error_code: 0x03,
                reason_phrase: e.to_string(),
            };
            let bytes = utils::create_full_message(ControlMessageType::PublishError, err);
            if let Err(e) = session.send_stream.send(&bytes).await {
                tracing::info!("send `Publish_Namespace_Error` failed: {}.", e.to_string());
            } else {
                tracing::info!("send `Publish_Namespace_Error` OK.");
            }
            return;
        }
        let result = result.unwrap();
        let (auth, _, _) = sequence_handler_util::resolve_param(result.parameters);
        // TODO: checks
        // authorized
        // uninterest namespace
        // malformed authorization token
        tracing::info!("Publish OK");
        let publish_ok = PublishOk {
            request_id: result.request_id,
            forward: result.forward,
            subscriber_priority: 128,
            group_order: result.group_order,
            filter_type: FilterType::LatestObject,
            start_location: result.largest_location,
            end_group: None,
            parameters: vec![],
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishOk, publish_ok);
        match session.send_stream.send(&bytes).await {
            Ok(_) => {
                let namespace = result.track_namespace_tuple.join("/");
                let session_event = SessionEvent::Publish(
                    namespace,
                    result.track_name,
                    result.track_alias,
                    result.group_order,
                    result.forward,
                    result.largest_location,
                    result.content_exists,
                    0,
                    0,
                );
                match session.event_sender.send(session_event) {
                    Ok(_) => tracing::info!("Message has been sent on sender."),
                    Err(_) => tracing::error!("failed to send publish namespace response message"),
                }
            }
            Err(_) => tracing::error!("failed to send publish namespace response message"),
        }
    }

    pub(crate) async fn publish_ok<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = PublishOk::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::PublishOk(
            result.request_id,
            result.group_order,
            result.subscriber_priority,
            result.forward,
            result.filter_type,
            result.start_location,
            result.end_group,
        );
        let sender = session.sender_map.lock().await.remove(&result.request_id);
        if let Some(sender) = sender {
            match sender.send(reponse) {
                Ok(_) => tracing::info!("Message has been sent on sender."),
                Err(_) => tracing::error!("failed to send publish namespace response message"),
            }
            tracing::info!("Message has been sent on sender.")
        } else {
            tracing::error!("Protocol violation");
        }
    }

    pub(crate) async fn publish_error<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = RequestError::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::PublishError(
            result.request_id,
            result.error_code,
            result.reason_phrase,
        );
        let sender = session.sender_map.lock().await.remove(&result.request_id);
        if let Some(sender) = sender {
            match sender.send(reponse) {
                Ok(_) => tracing::info!("Message has been sent on sender."),
                Err(_) => tracing::error!("failed to send publish namespace response message"),
            }
            tracing::info!("Message has been sent on sender.")
        } else {
            tracing::error!("Protocol violation");
        }
    }
}
