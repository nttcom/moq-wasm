use std::{
    pin::Pin,
    sync::{Arc, Weak},
};

use bytes::BytesMut;
use futures::{StreamExt, stream::FuturesUnordered};

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
                let mut futs = FuturesUnordered::new();
                loop {
                    if let Some(session) = inner_session.upgrade() {
                        tokio::select! {
                            bytes = session.receive_stream.receive() => {
                                if let Err(e) = bytes {
                                    tracing::error!("failed to receive message: {:?}", e);
                                    break;
                                }
                                let mut bytes_mut = BytesMut::from(bytes.unwrap().as_slice());
                                let message_type = get_message_type(&mut bytes_mut);
                                if let Err(e) = message_type.as_ref() {
                                    tracing::error!("Protocol violation: {:?}", e);
                                    break;
                                }
                                let result = Self::resolve_message(session, message_type.unwrap(), bytes_mut);
                                futs.push(result);
                            }
                            Some(_) = futs.next() => {
                                tracing::debug!("futus has been resolved.");
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

    async fn resolve_message<T: TransportProtocol>(
        session: Arc<InnerSession<T>>,
        message_type: ControlMessageType,
        bytes_mut: BytesMut,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
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
            ControlMessageType::PublishNamespace => Box::pin(
                PublishNamespaceHandler::publish_namespace(session, bytes_mut),
            ),
            ControlMessageType::PublishNamespaceOk => Box::pin(
                PublishNamespaceHandler::publish_namespace_ok(session, bytes_mut),
            ),
            ControlMessageType::PublishNamespaceError => Box::pin(
                PublishNamespaceHandler::publish_namespace_error(session, bytes_mut),
            ),
            ControlMessageType::PublishNamespaceDone => todo!(),
            ControlMessageType::PublishNamespaceCancel => todo!(),
            ControlMessageType::SubscribeNamespace => Box::pin(
                SubscribeNamespaceHandler::subscribe_namespace(session, bytes_mut),
            ),
            ControlMessageType::SubscribeNamespaceOk => Box::pin(
                SubscribeNamespaceHandler::subscribe_namespace_ok(session, bytes_mut),
            ),
            ControlMessageType::SubscribeNamespaceError => Box::pin(
                SubscribeNamespaceHandler::subscribe_namespace_error(session, bytes_mut),
            ),
            ControlMessageType::UnSubscribeNamespace => todo!(),
            _ => todo!(),
        }
    }
}
