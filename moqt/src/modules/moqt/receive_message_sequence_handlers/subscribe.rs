use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::moqt::{
        enums::{ResponseMessage, SessionEvent},
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                request_error::RequestError, subscribe::Subscribe, subscribe_ok::SubscribeOk,
            },
            moqt_message::MOQTMessage,
        },
        receive_message_sequence_handlers::sequence_handler_util,
        sessions::session_context::SessionContext,
        utils,
    },
};

pub(crate) struct SubscribeHandler;

impl SubscribeHandler {
    pub(crate) async fn subscribe<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = Subscribe::depacketize(&mut bytes_mut);
        if let Err(e) = result.as_ref() {
            let err = RequestError {
                // TODO: assign correct request id.
                request_id: 0,
                error_code: 0x03,
                reason_phrase: e.to_string(),
            };
            let bytes = utils::create_full_message(ControlMessageType::SubscribeError, err);
            match session.send_stream.send(&bytes).await {
                Ok(_) => tracing::info!("send `Subscribe_Namespace_Error` OK."),
                Err(e) => tracing::error!(
                    "send `Subscribe_Namespace_Error` failed: {}.",
                    e.to_string()
                ),
            }
        }
        let result = result.unwrap();
        let (auth, _, _) = sequence_handler_util::resolve_param(result.subscribe_parameters);

        // TODO: checks
        // authorized
        // uninterest namespace
        // malformed authorization token
        let track_namespace: Vec<String> = result.track_namespace;
        {
            let mut subscribed_namespaces = session.subscribed_namespaces.lock().await;
            for s in &track_namespace {
                if !subscribed_namespaces.insert(s.clone()) {
                    let err = RequestError {
                        // TODO: assign correct request id.
                        request_id: 0,
                        error_code: 0x03,
                        reason_phrase: format!("Namespace '{}' is already subscribed.", s),
                    };
                    let bytes = utils::create_full_message(ControlMessageType::SubscribeError, err);
                    match session.send_stream.send(&bytes).await {
                        Ok(_) => tracing::info!("send `Subscribe_Namespace_Error` OK."),
                        Err(e) => tracing::error!(
                            "send `Subscribe_Namespace_Error` failed: {}.",
                            e.to_string()
                        ),
                    }
                    return;
                }
            }
        }

        tracing::info!("Subscribe OK");
        let subscribe_namespace_ok = SubscribeOk {
            request_id: result.request_id,
            track_alias: result.track_alias,
            expires: 0,
            group_order: result.group_order,
            content_exists: false,
            largest_location: None,
            subscribe_parameters: vec![],
        };
        let bytes =
            utils::create_full_message(ControlMessageType::SubscribeOk, subscribe_namespace_ok);

        match session.send_stream.send(&bytes).await {
            Ok(_) => {
                let namespace = track_namespace.join("/");
                let session_event = SessionEvent::Subscribe(
                    namespace,
                    result.track_name,
                    result.track_alias,
                    result.subscriber_priority,
                    result.group_order,
                    false,
                    result.forward,
                    result.filter_type,
                    0,
                );
                match session.event_sender.send(session_event) {
                    Ok(_) => tracing::info!("Message has been sent on sender."),
                    Err(_) => {
                        tracing::error!("failed to send subscribe namespace response message")
                    }
                }
            }
            Err(_) => tracing::error!("failed to send subscribe namespace response message"),
        }
    }

    pub(crate) async fn subscribe_ok<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = SubscribeOk::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::SubscribeOk(
            result.request_id,
            result.track_alias,
            result.expires,
            result.group_order,
            result.content_exists,
            result.largest_location,
        );
        let sender = session.sender_map.lock().await.remove(&result.request_id);
        if let Some(sender) = sender {
            sender.send(reponse);
            tracing::info!("Message has been sent on sender.");
        } else {
            tracing::error!("Protocol violation");
        }
    }

    pub(crate) async fn subscribe_error<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = RequestError::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::SubscribeError(
            result.request_id,
            result.error_code,
            result.reason_phrase,
        );
        let sender = session.sender_map.lock().await.remove(&result.request_id);
        if let Some(sender) = sender {
            sender.send(reponse);
            tracing::info!("Message has been sent on sender.");
        } else {
            tracing::error!("Protocol violation");
        }
    }
}
