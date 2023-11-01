use num_enum::IntoPrimitive;

// 仮
#[derive(Debug, IntoPrimitive)]
#[repr(u8)]
pub enum TerminationErrorCode {
    SessionTerminated = 0x0,
    GenericError = 0x1,
    Unauthorized = 0x2,
    GoAway = 0x10,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnderlayType {
    QUIC,
    WebTransport,
    Both,
}
