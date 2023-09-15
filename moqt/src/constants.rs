// 仮
#[derive(Debug)]
pub enum TerminationErrorCode {
    SessionTerminated = 0x0,
    GenericError = 0x1,
    Unauthorized = 0x2,
    GoAway = 0x10,
}

#[derive(Debug)]
pub enum UnderlayType {
    QUIC,
    WebTransport,
    Both,
}
