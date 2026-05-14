use crate::modules::core::handler::{
    publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    unsubscribe::UnsubscribeHandler,
};

pub(crate) enum MoqtSessionEvent {
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    Publish(Box<dyn PublishHandler>),
    Subscribe(Box<dyn SubscribeHandler>),
    Unsubscribe(Box<dyn UnsubscribeHandler>),
    Disconnected(),
    ProtocolViolation(),
}

impl std::fmt::Debug for MoqtSessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            MoqtSessionEvent::PublishNamespace(_) => "PublishNamespace",
            MoqtSessionEvent::SubscribeNamespace(_) => "SubscribeNamespace",
            MoqtSessionEvent::Publish(_) => "Publish",
            MoqtSessionEvent::Subscribe(_) => "Subscribe",
            MoqtSessionEvent::Unsubscribe(_) => "Unsubscribe",
            MoqtSessionEvent::Disconnected() => "Disconnected",
            MoqtSessionEvent::ProtocolViolation() => "ProtocolViolation",
        };

        f.write_str(name)
    }
}
