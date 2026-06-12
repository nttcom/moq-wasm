use crate::modules::core::handler::{
    fetch::FetchHandler, publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    publish_namespace_done::PublishNamespaceDoneHandler, subscribe::SubscribeHandler,
    subscribe_namespace::SubscribeNamespaceHandler, unsubscribe::UnsubscribeHandler,
    unsubscribe_namespace::UnsubscribeNamespaceHandler,
};

pub(crate) enum MoqtSessionEvent {
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    PublishNamespaceDone(Box<dyn PublishNamespaceDoneHandler>),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    UnsubscribeNamespace(Box<dyn UnsubscribeNamespaceHandler>),
    Publish(Box<dyn PublishHandler>),
    Subscribe(Box<dyn SubscribeHandler>),
    Unsubscribe(Box<dyn UnsubscribeHandler>),
    Fetch(Box<dyn FetchHandler>),
    Disconnected(),
    ProtocolViolation(),
}

impl std::fmt::Debug for MoqtSessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            MoqtSessionEvent::PublishNamespace(_) => "PublishNamespace",
            MoqtSessionEvent::PublishNamespaceDone(_) => "PublishNamespaceDone",
            MoqtSessionEvent::SubscribeNamespace(_) => "SubscribeNamespace",
            MoqtSessionEvent::UnsubscribeNamespace(_) => "UnsubscribeNamespace",
            MoqtSessionEvent::Publish(_) => "Publish",
            MoqtSessionEvent::Subscribe(_) => "Subscribe",
            MoqtSessionEvent::Unsubscribe(_) => "Unsubscribe",
            MoqtSessionEvent::Fetch(_) => "Fetch",
            MoqtSessionEvent::Disconnected() => "Disconnected",
            MoqtSessionEvent::ProtocolViolation() => "ProtocolViolation",
        };

        f.write_str(name)
    }
}
