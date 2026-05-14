use std::sync::{Arc, Weak};

use tracing::{Instrument, Span};

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
                unsubscribe_handler::UnsubscribeHandler,
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

pub(crate) struct ControlMessageReceiveTask;

impl ControlMessageReceiveTask {
    pub(crate) fn run<T: TransportProtocol>(
        mut receive_stream: BiStreamReceiver<T>,
        session_context: Weak<SessionContext<T>>,
        receiver_span: Span,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Control Message Receiver")
            .spawn(
                async move {
                    loop {
                        if let Some(session) = session_context.upgrade() {
                            let received_message = match receive_stream.receive().await {
                                Some(Ok(received_message)) => {
                                    tracing::info!(message = ?received_message, "Message received");
                                    received_message
                                }
                                _ => {
                                    tracing::error!("Stream ended.");
                                    break;
                                }
                            };

                            match Self::resolve_message(session.clone(), received_message) {
                                DepacketizeResult::SessionEvent(event) => {
                                    if let Err(error) = session.event_sender.send(event) {
                                        tracing::error!("failed to send message: {:?}", error);
                                    }
                                }
                                DepacketizeResult::ResponseMessage(request_id, message) => {
                                    if let Some(sender) =
                                        session.sender_map.lock().await.remove(&request_id)
                                    {
                                        if let Err(error) = sender.send(message) {
                                            tracing::error!("failed to send message: {:?}", error);
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
                }
                .instrument(receiver_span),
            )
            .unwrap()
    }

    fn resolve_message<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        received_message: ReceivedMessage,
    ) -> DepacketizeResult<T> {
        tracing::debug!(message = ?received_message, "Event: message_type");
        match received_message {
            ReceivedMessage::Subscribe(subscribe) => {
                tracing::debug!("Event: Subscribe");
                let subscribe_handler = SubscribeHandler::new(session.clone(), subscribe);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Subscribe(subscribe_handler))
            }
            ReceivedMessage::Unsubscribe(unsubscribe) => {
                tracing::debug!("Event: Unsubscribe");
                let unsubscribe_handler = UnsubscribeHandler::new(session.clone(), unsubscribe);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Unsubscribe(unsubscribe_handler))
            }
            ReceivedMessage::SubscribeOk(subscribe_ok) => {
                tracing::debug!("Event: Subscribe ok");
                let request_id = subscribe_ok.request_id;
                let response = ResponseMessage::SubscribeOk(subscribe_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeError(subscribe_error) => {
                tracing::debug!("Event: Subscribe error");
                let response = ResponseMessage::SubscribeError(
                    subscribe_error.request_id,
                    subscribe_error.error_code,
                    subscribe_error.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(subscribe_error.request_id, response)
            }
            ReceivedMessage::Publish(publish) => {
                tracing::debug!("Event: Publish");
                let publish_handler = PublishHandler::new(session.clone(), publish);
                DepacketizeResult::SessionEvent(SessionEvent::<T>::Publish(publish_handler))
            }
            ReceivedMessage::PublishOk(publish_ok) => {
                tracing::debug!("Event: Publish ok");
                let request_id = publish_ok.request_id;
                let response = ResponseMessage::PublishOk(publish_ok);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishError(publish_error) => {
                tracing::debug!("Event: Publish error");
                let request_id = publish_error.request_id;
                let error_code = publish_error.error_code;
                let reason_phrase = publish_error.reason_phrase.clone();
                let response = ResponseMessage::PublishError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
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
                let response = ResponseMessage::PublishNamespaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::PublishNamespaceError(publish_namespace_error) => {
                tracing::debug!("Event: Publish namespace error");
                let request_id = publish_namespace_error.request_id;
                let error_code = publish_namespace_error.error_code;
                let reason_phrase = publish_namespace_error.reason_phrase.clone();
                let response =
                    ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
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
                let response = ResponseMessage::SubscribeNameSpaceOk(request_id);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            ReceivedMessage::SubscribeNamespaceError(subscribe_namespace_error) => {
                tracing::debug!("Event: Subscribe namespace error");
                let request_id = subscribe_namespace_error.request_id;
                let error_code = subscribe_namespace_error.error_code;
                let reason_phrase = subscribe_namespace_error.reason_phrase.clone();
                let response =
                    ResponseMessage::SubscribeNameSpaceError(request_id, error_code, reason_phrase);
                DepacketizeResult::ResponseMessage(request_id, response)
            }
            _ => todo!(),
        }
    }
}
