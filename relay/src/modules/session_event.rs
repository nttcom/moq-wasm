use crate::modules::core::handler::{
    publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    unsubscribe::UnsubscribeHandler,
};
use crate::modules::types::SessionId;

pub(crate) enum SessionEvent {
    PublishNameSpace(SessionId, Box<dyn PublishNamespaceHandler>),
    SubscribeNameSpace(SessionId, Box<dyn SubscribeNamespaceHandler>),
    Publish(SessionId, Box<dyn PublishHandler>),
    Subscribe(SessionId, Box<dyn SubscribeHandler>),
    Unsubscribe(SessionId, Box<dyn UnsubscribeHandler>),
    Disconnected(SessionId),
    ProtocolViolation(SessionId),
}
