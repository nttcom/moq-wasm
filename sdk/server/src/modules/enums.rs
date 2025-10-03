use uuid::Uuid;

// message alias
pub(crate) type RequestId = u64;
pub(crate) type TrackNamespaces = Vec<String>;
pub(crate) type GroupOrder = u8;
pub(crate) type IsContentExist = u8;
pub(crate) type IsForward = u8;
pub(crate) type SubscriberPriority = u8;
pub(crate) type FilterType = u64;

// parameter alias
pub(crate) type AuthorizationInfo = String;
pub(crate) type DeliveryTimeout = u64;
pub(crate) type MaxCacheDuration = u64;
pub(crate) type Parameters = (AuthorizationInfo, DeliveryTimeout, MaxCacheDuration);

pub(crate) enum SessionEvent {
    PublishNameSpace(Uuid, RequestId, TrackNamespaces, Parameters),
    SubscribeNameSpace(Uuid, RequestId, TrackNamespaces, Parameters),
    Publish(
        Uuid,
        RequestId,
        TrackNamespaces,
        GroupOrder,
        IsContentExist,
        IsForward,
        Parameters,
    ),
    Subscribe(
        Uuid,
        RequestId,
        TrackNamespaces,
        SubscriberPriority,
        GroupOrder,
        IsContentExist,
        IsForward,
        FilterType,
        Parameters,
    ),
}
