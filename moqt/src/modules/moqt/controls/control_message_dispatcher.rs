use std::sync::{Arc, Weak};

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::moqt::{
        messages::{
            control_message_type::ControlMessageType, control_messages::util::get_message_type,
        },
        sequence_handlers::{
            publish_namespace::PublishNamespaceHandler,
            subscribe_namespace_handler::SubscribeNamespaceHandler,
        },
        sessions::inner_session::InnerSession,
    },
};

pub(crate) struct ControlMessageDispatcher;

impl ControlMessageDispatcher {
    pub(crate) fn run<T: TransportProtocol>(
        inner_session: Weak<InnerSession<T>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Message Receiver")
            .spawn(async move {
                loop {
                    if let Some(session) = inner_session.upgrade() {
                        tracing::debug!("Session is alive.");

                        let bytes = match session.receive_stream.receive().await {
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
                                tracing::error!("Protocol violation: {:?}", e);
                                break;
                            }
                        };
                        let result = Self::resolve_message(session, message_type, bytes_mut).await;
                    } else {
                        tracing::error!("Session has been dropped.");
                        break;
                    }
                }
            })
            .unwrap()
    }

    async fn resolve_message<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        message_type: ControlMessageType,
        bytes_mut: BytesMut,
    ) {
        tracing::debug!("message_type: {:?}", message_type);
        match message_type {
            ControlMessageType::GoAway => todo!(),
            ControlMessageType::MaxSubscribeId => todo!(),
            ControlMessageType::RequestsBlocked => todo!(),
            ControlMessageType::Subscribe => todo!(),
            ControlMessageType::SubscribeOk => todo!(),
            ControlMessageType::SubscribeError => todo!(),
            ControlMessageType::SubscribeUpdate => todo!(),
            ControlMessageType::UnSubscribe => todo!(),
            ControlMessageType::PublishDone => todo!(),
            ControlMessageType::Publish => todo!(),
            ControlMessageType::PublishOk => todo!(),
            ControlMessageType::PublishError => todo!(),
            ControlMessageType::Fetch => todo!(),
            ControlMessageType::FetchOk => todo!(),
            ControlMessageType::FetchError => todo!(),
            ControlMessageType::FetchCancel => todo!(),
            ControlMessageType::TrackStatusRequest => todo!(),
            ControlMessageType::TrackStatus => todo!(),
            ControlMessageType::PublishNamespace => {
                PublishNamespaceHandler::publish_namespace(session, bytes_mut).await
            }
            ControlMessageType::PublishNamespaceOk => {
                PublishNamespaceHandler::publish_namespace_ok(session, bytes_mut).await
            }
            ControlMessageType::PublishNamespaceError => {
                PublishNamespaceHandler::publish_namespace_error(session, bytes_mut).await
            }
            ControlMessageType::PublishNamespaceDone => todo!(),
            ControlMessageType::PublishNamespaceCancel => todo!(),
            ControlMessageType::SubscribeNamespace => {
                SubscribeNamespaceHandler::subscribe_namespace(session, bytes_mut).await
            }
            ControlMessageType::SubscribeNamespaceOk => {
                SubscribeNamespaceHandler::subscribe_namespace_ok(session, bytes_mut).await
            }
            ControlMessageType::SubscribeNamespaceError => {
                SubscribeNamespaceHandler::subscribe_namespace_error(session, bytes_mut).await
            }
            ControlMessageType::UnSubscribeNamespace => todo!(),
            _ => todo!(),
        }
    }
}
