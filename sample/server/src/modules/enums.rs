use crate::modules::types::{
    DeliveryTimeout, GroupOrder, IsContentExist, IsForward, MaxCacheDuration,
    SessionId, SubscriberPriority, TrackAlias, TrackName, TrackNamespace, TrackNamespacePrefix,
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
    PublishNameSpace(SessionId, TrackNamespace),
    SubscribeNameSpace(SessionId, TrackNamespacePrefix),
    Publish(
        SessionId,
        TrackNamespace,
        TrackName,
        TrackAlias,
        GroupOrder,
        IsContentExist,
        Option<Location>,
        IsForward,
        DeliveryTimeout,
        MaxCacheDuration,
    ),
    Subscribe(
        SessionId,
        TrackNamespace,
        TrackName,
        TrackAlias,
        SubscriberPriority,
        GroupOrder,
        IsContentExist,
        IsForward,
        FilterType,
        DeliveryTimeout,
    ),
}
