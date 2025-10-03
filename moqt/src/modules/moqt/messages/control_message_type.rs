use num_enum::{IntoPrimitive, TryFromPrimitive};

#[derive(Debug, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy, Clone)]
#[repr(u8)]
pub enum ControlMessageType {
    // setup and dispose
    // less version than 10.
    // ClientSetup = 0x40,
    // ServerSetup = 0x41,
    ClientSetup = 0x20,
    ServerSetup = 0x21,
    GoAway = 0x10,

    MaxSubscribeId = 0x15,
    RequestsBlocked = 0x1a,

    // subscribe
    Subscribe = 0x03,
    SubscribeOk = 0x04,
    SubscribeError = 0x05,
    SubscribeUpdate = 0x02,
    UnSubscribe = 0x0a,

    // publish
    PublishDone = 0x0b,
    Publish = 0x1d,
    PublishOk = 0x1e,
    PublishError = 0x1f,

    // fetch
    Fetch = 0x16,
    FetchOk = 0x18,
    FetchError = 0x19,
    FetchCancel = 0x17,

    // track
    TrackStatusRequest = 0x0d,
    TrackStatus = 0x0e,

    // publish namespace
    PublishNamespace = 0x06,
    PublishNamespaceOk = 0x07,
    PublishNamespaceError = 0x08,
    PublishNamespaceDone = 0x09,
    PublishNamespaceCancel = 0x0c,

    // subscribe namespace
    SubscribeNamespace = 0x11,
    SubscribeNamespaceOk = 0x12,
    SubscribeNamespaceError = 0x13,
    UnSubscribeNamespace = 0x14,
}