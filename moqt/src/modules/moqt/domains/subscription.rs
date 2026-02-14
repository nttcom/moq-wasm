use crate::{
    DatagramReceiver, GroupOrder, StreamDataReceiver, TransportProtocol,
    modules::moqt::control_plane::messages::control_messages::{
        enums::ContentExists, subscribe_ok::SubscribeOk,
    },
};

pub enum DataReceiver<T: TransportProtocol> {
    Stream(StreamDataReceiver<T>),
    Datagram(DatagramReceiver<T>),
}

pub struct Subscription {
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub delivery_timeout: Option<u64>,
}

impl Subscription {
    pub(crate) fn new(subscribe_ok: SubscribeOk) -> Self {
        Self {
            track_alias: subscribe_ok.track_alias,
            expires: subscribe_ok.expires,
            group_order: subscribe_ok.group_order,
            content_exists: subscribe_ok.content_exists,
            delivery_timeout: None,
        }
    }
}
