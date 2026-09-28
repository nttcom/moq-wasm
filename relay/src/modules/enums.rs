// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.12
// PUBLISH_ERROR error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum PublishErrorCode {
    InternalError = 0x0,
    Unauthorized = 0x1,
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.25
// PUBLISH_NAMESPACE_ERROR error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum PublishNamespaceErrorCode {
    InternalError = 0x0,
    Unauthorized = 0x1,
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.30
// SUBSCRIBE_NAMESPACE_ERROR error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum SubscribeNamespaceErrorCode {
    Unauthorized = 0x1,
    NamespacePrefixOverlap = 0x5,
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.18
// FETCH_ERROR error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum FetchErrorCode {
    InternalError = 0x0,
    Unauthorized = 0x1,
    Timeout = 0x2,
    TrackDoesNotExist = 0x4,
    InvalidRange = 0x5,
    NoObjects = 0x6,
    InvalidJoiningRequestId = 0x7,
    MalformedTrack = 0x9,
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.9
// SUBSCRIBE_ERROR error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum SubscribeErrorCode {
    InternalError = 0x0,
    Unauthorized = 0x1,
    Timeout = 0x2,
    NotSupported = 0x3,
    TrackDoesNotExist = 0x4,
    MalformedAuthToken = 0x10,
}
