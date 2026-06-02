use crate::modules::core::handler::{
    fetch::FetchHandler, publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    unsubscribe::UnsubscribeHandler, unsubscribe_namespace::UnsubscribeNamespaceHandler,
};
use crate::modules::types::SessionId;

pub(crate) enum SessionEvent {
    PublishNameSpace(SessionId, Box<dyn PublishNamespaceHandler>),
    SubscribeNameSpace(SessionId, Box<dyn SubscribeNamespaceHandler>),
    UnsubscribeNameSpace(SessionId, Box<dyn UnsubscribeNamespaceHandler>),
    Publish(SessionId, Box<dyn PublishHandler>),
    Subscribe(SessionId, Box<dyn SubscribeHandler>),
    Unsubscribe(SessionId, Box<dyn UnsubscribeHandler>),
    Fetch(SessionId, Box<dyn FetchHandler>),
    Disconnected(SessionId),
    ProtocolViolation(SessionId),
}
