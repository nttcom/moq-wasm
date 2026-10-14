/// The request a `RequestErrorCode` is sent or received for; draft-14 numbers
/// the codes per `*_ERROR` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Subscribe,
    Publish,
    Fetch,
    TrackStatus,
    PublishNamespace,
    SubscribeNamespace,
}

/// Why a request was rejected (draft-ietf-moq-transport-14 §13.1.2 and
/// §13.1.4–§13.1.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestErrorCode {
    InternalError,
    Unauthorized,
    Timeout,
    NotSupported,
    TrackDoesNotExist,
    InvalidRange,
    NoObjects,
    InvalidJoiningRequestId,
    UnknownStatusInRange,
    MalformedTrack,
    MalformedAuthToken,
    ExpiredAuthToken,
    Uninterested,
    NamespacePrefixUnknown,
    NamespacePrefixOverlap,
}

/// PUBLISH_DONE status codes (draft-ietf-moq-transport-14 §13.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishDoneCode {
    InternalError,
    Unauthorized,
    TrackEnded,
    SubscriptionEnded,
    GoingAway,
    Expired,
    TooFarBehind,
    MalformedTrack,
}

/// Data stream reset codes (draft-ietf-moq-transport-14 §13.1.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStreamResetCode {
    InternalError,
    Cancelled,
    DeliveryTimeout,
    SessionClosed,
    MalformedTrack,
}
