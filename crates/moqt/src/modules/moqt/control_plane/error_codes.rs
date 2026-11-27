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
    GoingAway,
    ExcessiveLoad,
    DuplicateSubscription,
    NamespaceTooLarge,
    UnsupportedExtension,
    Redirect,
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
    UpdateFailed,
    ExcessiveLoad,
}

impl PublishDoneCode {
    /// The code's name in the drafts' PUBLISH_DONE code tables
    /// (draft-14 §13.1.3, draft-18 §15.10.3), e.g. `TRACK_ENDED`. Unlike the
    /// code number, it is the same on every draft.
    pub fn name(self) -> &'static str {
        match self {
            Self::InternalError => "INTERNAL_ERROR",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::TrackEnded => "TRACK_ENDED",
            Self::SubscriptionEnded => "SUBSCRIPTION_ENDED",
            Self::GoingAway => "GOING_AWAY",
            Self::Expired => "EXPIRED",
            Self::TooFarBehind => "TOO_FAR_BEHIND",
            Self::MalformedTrack => "MALFORMED_TRACK",
            Self::UpdateFailed => "UPDATE_FAILED",
            Self::ExcessiveLoad => "EXCESSIVE_LOAD",
        }
    }
}

/// Data stream reset codes (draft-ietf-moq-transport-14 §13.1.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStreamResetCode {
    InternalError,
    Cancelled,
    DeliveryTimeout,
    SessionClosed,
    MalformedTrack,
    GoingAway,
    TooFarBehind,
    UnknownObjectStatus,
    ExpiredAuthToken,
    ExcessiveLoad,
}

impl DataStreamResetCode {
    /// The code's name in the drafts' stream reset code tables
    /// (draft-14 §13.1.8, draft-18 §15.10.4), e.g. `CANCELLED`, the same on
    /// every draft.
    pub fn name(self) -> &'static str {
        match self {
            Self::InternalError => "INTERNAL_ERROR",
            Self::Cancelled => "CANCELLED",
            Self::DeliveryTimeout => "DELIVERY_TIMEOUT",
            Self::SessionClosed => "SESSION_CLOSED",
            Self::MalformedTrack => "MALFORMED_TRACK",
            Self::GoingAway => "GOING_AWAY",
            Self::TooFarBehind => "TOO_FAR_BEHIND",
            Self::UnknownObjectStatus => "UNKNOWN_OBJECT_STATUS",
            Self::ExpiredAuthToken => "EXPIRED_AUTH_TOKEN",
            Self::ExcessiveLoad => "EXCESSIVE_LOAD",
        }
    }
}
