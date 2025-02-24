use num_enum::{IntoPrimitive, TryFromPrimitive};

#[derive(Debug, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Clone)]
#[repr(u8)]
pub enum ControlMessageType {
    SubscribeUpdate = 0x02,
    Subscribe = 0x03,
    SubscribeOk = 0x04,
    SubscribeError = 0x05,
    Announce = 0x06,
    AnnounceOk = 0x07,
    AnnounceError = 0x08,
    UnAnnounce = 0x09,
    UnSubscribe = 0x0a,
    SubscribeDone = 0x0b,
    AnnounceCancel = 0x0c,
    TrackStatusRequest = 0x0d,
    TrackStatus = 0x0e,
    GoAway = 0x10,
    SubscribeNamespace = 0x11,
    SubscribeNamespaceOk = 0x12,
    SubscribeNamespaceError = 0x13,
    UnSubscribeNamespace = 0x14,
    MaxSubscribeId = 0x15,
    Fetch = 0x16,
    FetchCancel = 0x17,
    FetchOk = 0x18,
    FetchError = 0x19,
    ClientSetup = 0x40,
    ServerSetup = 0x41,
}

impl ControlMessageType {
    pub fn is_setup_message(&self) -> bool {
        matches!(
            self,
            ControlMessageType::ClientSetup | ControlMessageType::ServerSetup
        )
    }
    pub fn is_control_message(&self) -> bool {
        !self.is_setup_message()
    }
}
