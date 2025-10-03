#[derive(Clone)]
pub(crate) enum ReceiveEvent {
    Message(Vec<u8>),
    Error(),
}

// message aliases
pub type RequestId = u64;
pub type TrackNamespaces = Vec<String>;
pub type TrackAlias = u64;
pub type GroupOrder = u8;
pub type IsContentExist = u8;
pub type IsForward = u8;
pub type SubscriberPriority = u8;
pub type FilterType = u64;
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
    PublishNameSpace(RequestId, TrackNamespaces, Vec<Authorization>),
    SubscribeNameSpace(RequestId, TrackNamespaces, Vec<Authorization>),
    Publish(
        RequestId,
        TrackNamespaces,
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
        TrackNamespaces,
        SubscriberPriority,
        GroupOrder,
        IsContentExist,
        IsForward,
        FilterType,
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
    ),
}
