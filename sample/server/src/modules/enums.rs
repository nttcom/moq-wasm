use crate::modules::types::{
    Authorization, DeliveryTimeout, FilterType, GroupOrder, IsContentExist, IsForward,
    MaxCacheDuration, RequestId, SessionId, SubscriberPriority, TrackAlias, TrackName,
    TrackNamespace, TrackNamespacePrefix,
};

pub(crate) enum SessionEvent {
    PublishNameSpace(SessionId, TrackNamespace),
    SubscribeNameSpace(SessionId, TrackNamespacePrefix),
    Publish(
        SessionId,
        RequestId,
        TrackNamespace,
        TrackName,
        TrackAlias,
        GroupOrder,
        IsContentExist,
        IsForward,
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
        Vec<MaxCacheDuration>,
    ),
    Subscribe(
        SessionId,
        RequestId,
        TrackNamespace,
        SubscriberPriority,
        GroupOrder,
        IsContentExist,
        IsForward,
        FilterType,
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
    ),
}
