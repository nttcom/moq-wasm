use std::{fmt::format, sync::Arc};

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::moqt::{
        enums::{ResponseMessage, SessionEvent},
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                namespace_ok::NamespaceOk, request_error::RequestError,
                subscribe_namespace::SubscribeNamespace,
            },
            moqt_message::MOQTMessage,
        },
        sequence_handlers::sequence_handler_util,
        sessions::inner_session::InnerSession,
        utils,
    },
};

pub(crate) struct SubscribeNamespaceHandler;

impl SubscribeNamespaceHandler {
    pub(crate) async fn subscribe_namespace<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = SubscribeNamespace::depacketize(&mut bytes_mut);
        if let Err(e) = result.as_ref() {
            let err = RequestError {
                // TODO: assign correct request id.
                request_id: 0,
                error_code: 0x03,
                reason_phrase: e.to_string(),
            };
            let bytes =
                utils::create_full_message(ControlMessageType::SubscribeNamespaceError, err);
            match session.send_stream.send(&bytes).await {
                Ok(_) => tracing::info!("send `Subscribe_Namespace_Error` OK."),
                Err(e) => tracing::error!(
                    "send `Subscribe_Namespace_Error` failed: {}.",
                    e.to_string()
                ),
            }
        }
        let result = result.unwrap();
        let (auth, _, _) = sequence_handler_util::resolve_param(result.parameters);

        // TODO: checks
        // authorized
        // uninterest namespace
        // malformed authorization token
        let track_namespace = result.track_namespace_prefix;
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
                    let bytes = utils::create_full_message(
                        ControlMessageType::SubscribeNamespaceError,
                        err,
                    );
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

        tracing::info!("Subscribe namespace OK");
        let subscribe_namespace_ok = NamespaceOk {
            request_id: result.request_id,
        };
        let bytes = utils::create_full_message(
            ControlMessageType::SubscribeNamespaceOk,
            subscribe_namespace_ok,
        );

        match session.send_stream.send(&bytes).await {
            Ok(_) => {
                let session_event = SessionEvent::SubscribeNameSpace(track_namespace);
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

    pub(crate) async fn subscribe_namespace_ok<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = NamespaceOk::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::SubscribeNameSpaceOk(result.request_id);
        let sender = session.sender_map.lock().await.remove(&result.request_id);
        if let Some(sender) = sender {
            sender.send(reponse);
            tracing::info!("Message has been sent on sender.");
        } else {
            tracing::error!("Protocol violation");
        }
    }

    pub(crate) async fn subscribe_namespace_error<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = RequestError::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::SubscribeNameSpaceError(
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
