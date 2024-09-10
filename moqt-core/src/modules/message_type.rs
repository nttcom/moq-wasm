use num_enum::{IntoPrimitive, TryFromPrimitive};

#[derive(Debug, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Clone)]
#[repr(u8)]
pub enum MessageType {
    ObjectWithPayloadLength = 0x00,
    ObjectWithoutPayloadLength = 0x02,
    Subscribe = 0x03,
    SubscribeOk = 0x04,
    SubscribeError = 0x05,
    Announce = 0x06,
    AnnounceOk = 0x07,
    AnnounceError = 0x08,
    UnAnnounce = 0x09,
    UnSubscribe = 0x0a,
    SubscribeFin = 0x0b,
    SubscribeRst = 0x0c,
    GoAway = 0x10,
    ClientSetup = 0x40,
    ServerSetup = 0x41,
}

impl MessageType {
    pub fn is_setup_message(&self) -> bool {
        matches!(self, MessageType::ClientSetup | MessageType::ServerSetup)
    }
    pub fn is_object(&self) -> bool {
        matches!(
            self,
            MessageType::ObjectWithPayloadLength | MessageType::ObjectWithoutPayloadLength
        )
    }
    pub fn is_control_message(&self) -> bool {
        !self.is_setup_message() && !self.is_object()
    }
}
