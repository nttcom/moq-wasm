// message aliases
pub type RequestId = u64;
pub type TrackNamespace = String;
pub type TrackAlias = u64;
pub type GroupOrder = u8;
pub type IsContentExist = u8;
pub type IsForward = u8;
pub type SubscriberPriority = u8;
pub type FilterType = u64;
pub(crate) type ErrorCode = u64;
pub(crate) type ErrorPhrase = String;

// parameters aliases
// appear in
// CLIENT_SETUP, SERVER_SETUP, PUBLISH, SUBSCRIBE, SUBSCRIBE_UPDATE,
// SUBSCRIBE_NAMESPACE, PUBLISH_NAMESPACE, TRACK_STATUS or FETCH
pub type Authorization = String;
// TRACK_STATUS, TRACK_STATUS_OK, PUBLISH, PUBLISH_OK, SUBSCRIBE,
// SUBSCRIBE_OK, or SUBSCRIBE_UDPATE message.
pub type DeliveryTimeout = u64;
// PUBLISH, SUBSCRIBE_OK, FETCH_OK or TRACK_STATUS_OK
pub type MaxCacheDuration = u64;

#[derive(Clone)]
pub enum SessionEvent {
    PublishNamespace(TrackNamespace),
    SubscribeNameSpace(TrackNamespace),
    Publish(
        RequestId,
        TrackNamespace,
        TrackAlias,
        GroupOrder,
        IsContentExist,
        IsForward,
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
        Vec<MaxCacheDuration>,
    ),
    Subscribe(
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
    FatalError(),
}

pub(crate) enum ResponseMessage {
    SubscribeNameSpaceOk(RequestId),
    SubscribeNameSpaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishNamespaceOk(RequestId),
    PublishNamespaceError(RequestId, ErrorCode, ErrorPhrase),
}
