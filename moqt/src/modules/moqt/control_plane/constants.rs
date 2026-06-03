use num_enum::IntoPrimitive;

// for draft-ietf-moq-transport-10
pub const MOQ_TRANSPORT_VERSION: u32 = 0xff00000e;

/// Session termination error codes, draft-ietf-moq-transport-14 §13.1.1.
#[derive(Debug, IntoPrimitive, PartialEq, Clone, Copy)]
#[repr(u32)]
#[allow(dead_code)]
pub enum TerminationErrorCode {
    NoError = 0x0,
    InternalError = 0x1,
    Unauthorized = 0x2,
    ProtocolViolation = 0x3,
    InvalidRequestId = 0x4,
    DuplicateTrackAlias = 0x5,
    KeyValueFormattingError = 0x6,
    TooManyRequests = 0x7,
    InvalidPath = 0x8,
    MalformedPath = 0x9,
    GoawayTimeout = 0x10,
    ControlMessageTimeout = 0x11,
    DataStreamTimeout = 0x12,
    AuthTokenCacheOverflow = 0x13,
    DuplicateAuthTokenAlias = 0x14,
    VersionNegotiationFailed = 0x15,
    MalformedAuthToken = 0x16,
    UnknownAuthTokenAlias = 0x17,
    ExpiredAuthToken = 0x18,
    InvalidAuthority = 0x19,
    MalformedAuthority = 0x1A,
}
