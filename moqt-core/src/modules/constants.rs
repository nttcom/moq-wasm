use num_enum::IntoPrimitive;

// for draft-ietf-moq-transport-07
pub const MOQ_TRANSPORT_VERSION: u32 = 0xff000007;

#[derive(Debug, IntoPrimitive, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum TerminationErrorCode {
    NoError = 0x0,
    InternalError = 0x1,
    Unauthorized = 0x2,
    ProtocolViolation = 0x3,
    DuplicateTrackAlias = 0x4,
    ParameterLengthMismatch = 0x5,
    TooManySubscribes = 0x6,
    GoAwayTimeout = 0x10,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnderlayType {
    QUIC,
    WebTransport,
    Both,
}
