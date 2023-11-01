use num_enum::{IntoPrimitive, TryFromPrimitive};

#[derive(Debug, PartialEq, Eq, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum MessageType {
    Object = 0x00,
    Setup = 0x01,
    SubscribeRequest = 0x03,
    SubscribeOk = 0x04,
    SubscribeError = 0x05,
    Announce = 0x06,
    AnnounceOk = 0x07,
    AnnounceError = 0x08,
    GoAway = 0x10,
}
