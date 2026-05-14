use crate::modules::core::handler::{
    publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    unsubscribe::UnsubscribeHandler,
};

pub(crate) enum SessionEvent {
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    Publish(Box<dyn PublishHandler>),
    Subscribe(Box<dyn SubscribeHandler>),
    Unsubscribe(Box<dyn UnsubscribeHandler>),
    Disconnected(),
    ProtocolViolation(),
}

impl std::fmt::Debug for SessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            SessionEvent::PublishNamespace(_) => "PublishNamespace",
            SessionEvent::SubscribeNamespace(_) => "SubscribeNamespace",
            SessionEvent::Publish(_) => "Publish",
            SessionEvent::Subscribe(_) => "Subscribe",
            SessionEvent::Unsubscribe(_) => "Unsubscribe",
            SessionEvent::Disconnected() => "Disconnected",
            SessionEvent::ProtocolViolation() => "ProtocolViolation",
        };

        f.write_str(name)
    }
}
