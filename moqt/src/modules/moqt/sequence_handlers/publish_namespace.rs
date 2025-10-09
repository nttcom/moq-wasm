use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::moqt::{
        enums::{ResponseMessage, SessionEvent},
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                namespace_ok::NamespaceOk, publish_namespace::PublishNamespace,
                request_error::RequestError,
            },
            moqt_message::MOQTMessage,
        },
        sequence_handlers::sequence_handler_util,
        sessions::inner_session::InnerSession,
        utils,
    },
};

pub(crate) struct PublishNamespaceHandler;

impl PublishNamespaceHandler {
    pub(crate) async fn publish_namespace<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        tracing::info!("Publish namespace");
        let result = PublishNamespace::depacketize(&mut bytes_mut);
        if let Err(e) = result.as_ref() {
            tracing::warn!("Error has detected.");
            let err = RequestError {
                // TODO: assign correct request id.
                request_id: 0,
                error_code: 0x03,
                reason_phrase: e.to_string(),
            };
            let bytes = utils::create_full_message(ControlMessageType::PublishNamespaceError, err);
            if let Err(e) = session
                .send_stream
                .send(&bytes)
                .await {
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
        tracing::info!("Publish namespace OK");
        let publish_namespace_ok = NamespaceOk {
            request_id: result.request_id,
        };
        let bytes = utils::create_full_message(
            ControlMessageType::PublishNamespaceOk,
            publish_namespace_ok,
        );
        match session.send_stream.send(&bytes).await {
            Ok(_) => {
                let namespace = result.track_namespace.join("/");
                let session_event = SessionEvent::PublishNamespace(namespace);
                match session.event_sender.send(session_event) {
                    Ok(_) => tracing::info!("Message has been sent on sender."),
                    Err(_) => tracing::error!("failed to send publish namespace response message"),
                }
            }
            Err(_) => tracing::error!("failed to send publish namespace response message"),
        }
    }

    pub(crate) async fn publish_namespace_ok<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = NamespaceOk::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::PublishNamespaceOk(result.request_id);
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

    pub(crate) async fn publish_namespace_error<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        mut bytes_mut: BytesMut,
    ) {
        let result = RequestError::depacketize(&mut bytes_mut).unwrap();
        let reponse = ResponseMessage::PublishNamespaceError(
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
