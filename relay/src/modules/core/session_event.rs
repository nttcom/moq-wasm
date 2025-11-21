use crate::modules::core::handler::{
    publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
};

pub(crate) enum SessionEvent {
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    Publish(Box<dyn PublishHandler>),
    Subscribe(Box<dyn SubscribeHandler>),
    ProtocolViolation(),
}
