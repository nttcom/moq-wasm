use std::sync::{Arc, Weak};

use bytes::BytesMut;

use crate::{
    SessionEvent, StreamReceiver, TransportProtocol,
    modules::moqt::control_plane::{
        enums::ResponseMessage,
        handler::{
            publish_handler::PublishHandler, publish_namespace_handler::PublishNamespaceHandler,
            subscribe_handler::SubscribeHandler,
            subscribe_namespace_handler::SubscribeNamespaceHandler,
        },
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                namespace_ok::NamespaceOk, publish::Publish, publish_namespace::PublishNamespace,
                publish_ok::PublishOk, request_error::RequestError, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
                util::get_message_type,
            },
        },
        models::session_context::SessionContext,
    },
};

enum DepacketizeResult<T: TransportProtocol> {
    SessionEvent(SessionEvent<T>),
    ResponseMessage(u64, ResponseMessage),
}

pub(crate) struct ControlMessageReceiveThread;

impl ControlMessageReceiveThread {
    pub(crate) fn run<T: TransportProtocol>(
        receive_stream: StreamReceiver<T>,
        session_context: Weak<SessionContext<T>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Message Receiver")
            .spawn(async move {
                loop {
                    if let Some(session) = session_context.upgrade() {
                        tracing::debug!("Session is alive.");

                        let bytes = match receive_stream.receive().await {
                            Ok(b) => b,
                            Err(e) => {
                                tracing::error!("failed to receive message: {:?}", e);
                                break;
                            }
                        };
                        tracing::info!("Message has been received.");
                        let mut bytes_mut = BytesMut::from(bytes.as_slice());
                        let message_type = match get_message_type(&mut bytes_mut) {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::error!("Receiving error: {:?}", e);
                                break;
                            }
                        };
                        let depack_result =
                            Self::resolve_message(session.clone(), message_type, bytes_mut);
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
                        tracing::error!("Session has been dropped.");
                        break;
                    }
                }
            })
            .unwrap()
    }

    fn resolve_message<T: TransportProtocol>(
        session: Arc<SessionContext<T>>,
        message_type: ControlMessageType,
        mut bytes_mut: BytesMut,
    ) -> DepacketizeResult<T> {
        tracing::debug!("Event: message_type: {:?}", message_type);
        match message_type {
            ControlMessageType::GoAway => todo!(),
            ControlMessageType::MaxSubscribeId => todo!(),
            ControlMessageType::RequestsBlocked => todo!(),
            ControlMessageType::Subscribe => {
                tracing::debug!("Event: Subscribe");
                let result = Subscribe::decode(&mut bytes_mut);
                if result.is_none() {
                    tracing::warn!("Error has detected.");
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::ProtocolViolation())
                } else {
                    let result = result.unwrap();
                    let subscribe: SubscribeHandler<T> =
                        SubscribeHandler::new(session.clone(), result);
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::Subscribe(subscribe))
                }
            }
            ControlMessageType::SubscribeOk => {
                tracing::debug!("Event: Subscribe ok");
                let result = match SubscribeOk::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let request_id = result.request_id;
                let reponse = ResponseMessage::SubscribeOk(result);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ControlMessageType::SubscribeError => {
                tracing::debug!("Event: Subscribe error");
                let result = match RequestError::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let reponse = ResponseMessage::SubscribeError(
                    result.request_id,
                    result.error_code,
                    result.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(result.request_id, reponse)
            }
            ControlMessageType::SubscribeUpdate => todo!(),
            ControlMessageType::UnSubscribe => todo!(),
            ControlMessageType::PublishDone => todo!(),
            ControlMessageType::Publish => {
                tracing::debug!("Event: Publish");
                let result = Publish::decode(&mut bytes_mut);
                if result.is_none() {
                    tracing::error!("Protocol violation is detected.");
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::ProtocolViolation())
                } else {
                    let result = result.unwrap();
                    let pub_handler = PublishHandler::new(session.clone(), result);
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::Publish(pub_handler))
                }
            }
            ControlMessageType::PublishOk => {
                tracing::debug!("Event: Publish ok");
                let result = match PublishOk::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let request_id = result.request_id;
                let reponse = ResponseMessage::PublishOk(result);
                DepacketizeResult::ResponseMessage(request_id, reponse)
            }
            ControlMessageType::PublishError => {
                tracing::debug!("Event: Publish error");
                let result = match RequestError::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let reponse = ResponseMessage::PublishError(
                    result.request_id,
                    result.error_code,
                    result.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(result.request_id, reponse)
            }
            ControlMessageType::Fetch => todo!(),
            ControlMessageType::FetchOk => todo!(),
            ControlMessageType::FetchError => todo!(),
            ControlMessageType::FetchCancel => todo!(),
            ControlMessageType::TrackStatusRequest => todo!(),
            ControlMessageType::TrackStatus => todo!(),
            ControlMessageType::PublishNamespace => {
                tracing::debug!("Event: Publish namespace");
                let result = PublishNamespace::decode(&mut bytes_mut);
                if result.is_none() {
                    tracing::error!("Protocol violation is detected.");
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::ProtocolViolation())
                } else {
                    let result = result.unwrap();
                    let pub_ns_handler = PublishNamespaceHandler::new(session.clone(), result);
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::PublishNamespace(
                        pub_ns_handler,
                    ))
                }
            }
            ControlMessageType::PublishNamespaceOk => {
                tracing::debug!("Event: Publish namespace ok");
                let result = match NamespaceOk::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let reponse = ResponseMessage::PublishNamespaceOk(result.request_id);
                DepacketizeResult::ResponseMessage(result.request_id, reponse)
            }
            ControlMessageType::PublishNamespaceError => {
                tracing::debug!("Event: Publish namespace error");
                let result = match RequestError::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let reponse = ResponseMessage::PublishNamespaceError(
                    result.request_id,
                    result.error_code,
                    result.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(result.request_id, reponse)
            }
            ControlMessageType::PublishNamespaceDone => todo!(),
            ControlMessageType::PublishNamespaceCancel => todo!(),
            ControlMessageType::SubscribeNamespace => {
                tracing::debug!("Event: Subscribe namespace");
                let result = SubscribeNamespace::decode(&mut bytes_mut);
                if result.is_none() {
                    tracing::error!("Protocol violation is detected.");
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::ProtocolViolation())
                } else {
                    let result = result.unwrap();
                    let sub_ns_handler = SubscribeNamespaceHandler::new(session.clone(), result);
                    DepacketizeResult::SessionEvent(SessionEvent::<T>::SubscribeNameSpace(
                        sub_ns_handler,
                    ))
                }
            }
            ControlMessageType::SubscribeNamespaceOk => {
                tracing::debug!("Event: Subscribe namespace ok");
                let result = match NamespaceOk::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let reponse = ResponseMessage::SubscribeNameSpaceOk(result.request_id);
                DepacketizeResult::ResponseMessage(result.request_id, reponse)
            }
            ControlMessageType::SubscribeNamespaceError => {
                tracing::debug!("Event: Subscribe namespace error");
                let result = match RequestError::decode(&mut bytes_mut) {
                    Some(v) => v,
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return DepacketizeResult::SessionEvent(
                            SessionEvent::<T>::ProtocolViolation(),
                        );
                    }
                };
                let reponse = ResponseMessage::SubscribeNameSpaceError(
                    result.request_id,
                    result.error_code,
                    result.reason_phrase,
                );
                DepacketizeResult::ResponseMessage(result.request_id, reponse)
            }
            ControlMessageType::UnSubscribeNamespace => todo!(),
            _ => todo!(),
        }
    }
}
