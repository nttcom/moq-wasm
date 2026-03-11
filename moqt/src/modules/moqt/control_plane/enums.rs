use crate::{
    TransportProtocol,
    modules::moqt::control_plane::{
        control_messages::messages::{publish_ok::PublishOk, subscribe_ok::SubscribeOk},
        handler::{
            publish_handler::PublishHandler, publish_namespace_handler::PublishNamespaceHandler,
            subscribe_handler::SubscribeHandler,
            subscribe_namespace_handler::SubscribeNamespaceHandler,
        },
    },
};

// message aliases
pub type RequestId = u64;

pub(crate) type ErrorCode = u64;
pub(crate) type ErrorPhrase = String;

#[derive(Clone, Debug)]
pub enum SessionEvent<T: TransportProtocol> {
    PublishNamespace(PublishNamespaceHandler<T>),
    SubscribeNameSpace(SubscribeNamespaceHandler<T>),
    Publish(PublishHandler<T>),
    Subscribe(SubscribeHandler<T>),
    ProtocolViolation(),
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum ResponseMessage {
    SubscribeNameSpaceOk(RequestId),
    SubscribeNameSpaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishNamespaceOk(RequestId),
    PublishNamespaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishOk(PublishOk),
    PublishError(RequestId, ErrorCode, ErrorPhrase),
    SubscribeOk(SubscribeOk),
    SubscribeError(RequestId, ErrorCode, ErrorPhrase),
}
