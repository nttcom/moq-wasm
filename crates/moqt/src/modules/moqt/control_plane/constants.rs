// for draft-ietf-moq-transport-10
pub const MOQ_TRANSPORT_VERSION: u32 = 0xff00000e;

/// Session termination error codes (draft-ietf-moq-transport-14 §13.1.1).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TerminationErrorCode {
    NoError,
    InternalError,
    Unauthorized,
    ProtocolViolation,
    InvalidRequestId,
    DuplicateTrackAlias,
    KeyValueFormattingError,
    TooManyRequests,
    InvalidPath,
    MalformedPath,
    GoawayTimeout,
    ControlMessageTimeout,
    DataStreamTimeout,
    AuthTokenCacheOverflow,
    DuplicateAuthTokenAlias,
    VersionNegotiationFailed,
    MalformedAuthToken,
    UnknownAuthTokenAlias,
    ExpiredAuthToken,
    InvalidAuthority,
    MalformedAuthority,
}
