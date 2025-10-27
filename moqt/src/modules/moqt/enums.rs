use crate::modules::moqt::messages::control_messages::{
    enums::FilterType, group_order::GroupOrder, location::Location,
};

// message aliases
pub type RequestId = u64;
pub type TrackNamespace = String;
pub type TrackAlias = u64;
pub type ContentExists = bool;
pub type Forward = bool;
pub type SubscriberPriority = u8;
pub type Expires = u64;
pub type TrackName = String;
pub type EndGroup = u64;

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

#[derive(Clone, Debug)]
pub enum SessionEvent {
    PublishNamespace(TrackNamespace),
    SubscribeNameSpace(TrackNamespace),
    Publish(
        TrackNamespace,
        TrackName,
        TrackAlias,
        GroupOrder,
        ContentExists,
        Option<Location>,
        Forward,
        DeliveryTimeout,
        MaxCacheDuration,
    ),
    Subscribe(
        TrackNamespace,
        TrackName,
        TrackAlias,
        SubscriberPriority,
        GroupOrder,
        ContentExists,
        Forward,
        FilterType,
        DeliveryTimeout,
    ),
    FatalError(),
}

pub(crate) enum ResponseMessage {
    SubscribeNameSpaceOk(RequestId),
    SubscribeNameSpaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishNamespaceOk(RequestId),
    PublishNamespaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishOk(
        RequestId,
        GroupOrder,
        SubscriberPriority,
        Forward,
        FilterType,
        Option<Location>,
        Option<EndGroup>,
    ),
    PublishError(RequestId, ErrorCode, ErrorPhrase),
    SubscribeOk(
        RequestId,
        TrackAlias,
        Expires,
        GroupOrder,
        ContentExists,
        Option<Location>,
    ),
    SubscribeError(RequestId, ErrorCode, ErrorPhrase),
}

pub enum StreamMessage {
    Header(),
    ObjectField
}

