use std::sync::{Arc, Weak};

use crate::{
    SessionEvent, TransportProtocol,
    modules::moqt::{
        control_plane::{
            enums::ResponseMessage,
            handler::{
                publish_handler::PublishHandler,
                publish_namespace_handler::PublishNamespaceHandler,
                subscribe_handler::SubscribeHandler,
                subscribe_namespace_handler::SubscribeNamespaceHandler,
            },
        },
        data_plane::streams::stream::{
            received_message::ReceivedMessage, stream_receiver::BiStreamReceiver,
        },
        domains::session_context::SessionContext,
    },
};

enum DepacketizeResult<T: TransportProtocol> {
    SessionEvent(SessionEvent<T>),
    ResponseMessage(u64, ResponseMessage),
}

pub(crate) struct ControlMessageReceiveThread;

impl ControlMessageReceiveThread {
    pub(crate) fn run<T: TransportProtocol>(
        mut receive_stream: BiStreamReceiver<T>,
        session_context: Weak<SessionContext<T>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Message Receiver")
            .spawn(async move {
                loop {
                    if let Some(session) = session_context.upgrade() {
                        tracing::debug!("Session is alive.");

                        let received_message = match receive_stream.receive().await {
                            Some(Ok(b)) => b,
                            _ => {
                                tracing::error!("Stream ended.");
                                break;
                            }
                        };
                        tracing::info!("Message received.");
                        let depack_result =
                            Self::resolve_message(session.clone(), received_message);
                        match depack_result {
                            DepacketizeResult::SessionEvent(event) => {
                                if let Err(e) = session.event_sender.send(event) {
                                    tracing::error!("failed to send message: {:?}", e);
                                }
                            }
                            DepacketizeResult::ResponseMessage(request_id, message) => {
                                if let Some(sender) =
                                    session.sender_map.lock().await.remove(&request_id)
                                {
                                    if let Err(e) = sender.send(message) {
                                        tracing::error!("failed to send message: {:?}", e);
                                    }
                                } else {
                                    tracing::error!("Protocol violation");
                                }
                            }
                        }
                    } else {
                        tracing::error!("Session dropped.");
                        break;
                    }
                }
            })
            .unwrap()
    }

    fn resolve_message<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        received_message: ReceivedMessage,
    ) -> DepacketizeResult<T> {
        tracing::debug!("Event: message_type: {:?}", received_message);
        match received_message {
            ReceivedMessage::Subscribe(subscribe) => {
                tracing::debug!("Event: Subscribe");
                let subscribe_handler = SubscribeHandler::new(session.clone(), subscribe);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Subscribe(subscribe_handler))
            }
            ReceivedMessage::SubscribeOk(subscribe_ok) => {
                tracing::debug!("Event: Subscribe ok");
                let request_id = subscribe_ok.request_id;
                let reponse = ResponseMessage::SubscribeOk(subscribe_ok);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ReceivedMessage::SubscribeError(subscribe_error) => {
                tracing::debug!("Event: Subscribe error");
                let reponse = ResponseMessage::SubscribeError(
                    subscribe_error.request_id,
                    subscribe_error.error_code,
                    subscribe_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(subscribe_error.request_id, reponse)
            }
            ReceivedMessage::Publish(publish) => {
                tracing::debug!("Event: Publish");
                let publish_handler = PublishHandler::new(session.clone(), publish);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Publish(publish_handler))
            }
            ReceivedMessage::PublishOk(publish_ok) => {
                tracing::debug!("Event: Publish ok");
                let request_id = publish_ok.request_id;
                let reponse = ResponseMessage::PublishOk(publish_ok);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ReceivedMessage::PublishError(publish_error) => {
                tracing::debug!("Event: Publish error");
                let request_id = publish_error.request_id;
                let error_code = publish_error.error_code;
                let reason_phrase = publish_error.reason_phrase.clone();
                let reponse = ResponseMessage::PublishError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ReceivedMessage::PublishNamespace(publish_namespace) => {
                tracing::debug!("Event: Publish namespace");
                let publish_namespace_handler =
                    PublishNamespaceHandler::new(session.clone(), publish_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::PublishNamespace(
                    publish_namespace_handler,
                ))
            }
            ReceivedMessage::PublishNamespaceOk(publish_namespace_ok) => {
                tracing::debug!("Event: Publish namespace ok");
                let request_id = publish_namespace_ok.request_id;
                let reponse = ResponseMessage::PublishNamespaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ReceivedMessage::PublishNamespaceError(publish_namespace_error) => {
                tracing::debug!("Event: Publish namespace error");
                let request_id = publish_namespace_error.request_id;
                let error_code = publish_namespace_error.error_code;
                let reason_phrase = publish_namespace_error.reason_phrase.clone();
                let reponse =
                    ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ReceivedMessage::SubscribeNamespace(subscribe_namespace) => {
                tracing::debug!("Event: Subscribe namespace");
                let subscribe_namespace_handler =
                    SubscribeNamespaceHandler::new(session.clone(), subscribe_namespace);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::SubscribeNameSpace(
                    subscribe_namespace_handler,
                ))
            }
            ReceivedMessage::SubscribeNamespaceOk(subscribe_namespace_ok) => {
                tracing::debug!("Event: Subscribe namespace ok");
                let request_id = subscribe_namespace_ok.request_id;
                let reponse = ResponseMessage::SubscribeNameSpaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ReceivedMessage::SubscribeNamespaceError(subscribe_namespace_error) => {
                tracing::debug!("Event: Subscribe namespace error");
                let request_id = subscribe_namespace_error.request_id;
                let error_code = subscribe_namespace_error.error_code;
                let reason_phrase = subscribe_namespace_error.reason_phrase.clone();
                let reponse =
                    ResponseMessage::SubscribeNameSpaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            _ => todo!(),
        }
    }
}
