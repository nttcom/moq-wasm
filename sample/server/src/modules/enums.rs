use crate::modules::{
    core::handler::{
        publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
        subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    },
    types::{
        DeliveryTimeout, GroupOrder, IsContentExist, IsForward, MaxCacheDuration, SessionId,
        SubscriberPriority, TrackAlias, TrackName, TrackNamespace, TrackNamespacePrefix,
    },
};

pub(crate) struct Location {
    pub(crate) group_id: u64,
    pub(crate) object_id: u64,
}

pub enum FilterType {
    LatestGroup = 0x1,
    LatestObject = 0x2,
    AbsoluteStart = 0x3,
    AbsoluteRange = 0x4,
}

pub(crate) enum MOQTMessageReceived {
    PublishNameSpace(SessionId, Box<dyn PublishNamespaceHandler>),
    SubscribeNameSpace(SessionId, Box<dyn SubscribeNamespaceHandler>),
    Publish(SessionId, Box<dyn PublishHandler>),
    Subscribe(SessionId, Box<dyn SubscribeHandler>),
    ProtocolViolation(),
}
