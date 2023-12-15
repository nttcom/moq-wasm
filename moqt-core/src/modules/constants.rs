use num_enum::IntoPrimitive;

// for draft-ietf-moq-transport-01
pub const MOQ_TRANSPORT_VERSION: u32 = 0xff000001;

// tmp
#[derive(Debug, IntoPrimitive)]
#[repr(u8)]
pub enum TerminationErrorCode {
    NoError = 0x0,
    GenericError = 0x1,
    Unauthorized = 0x2,
    ProtocolViolation = 0x3,
    GoAwayTimeout = 0x10,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnderlayType {
    QUIC,
    WebTransport,
    Both,
}
