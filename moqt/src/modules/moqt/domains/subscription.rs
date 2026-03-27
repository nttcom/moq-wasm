use std::sync::Arc;

use crate::{
    DatagramReceiver, GroupOrder, StreamDataReceiver, TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                parameters::content_exists::ContentExists, subscribe_ok::SubscribeOk,
            },
            threads::enums::StreamWithObject,
        },
        domains::session_context::SessionContext,
    },
};

pub enum DataReceiver<T: TransportProtocol> {
    Stream(StreamDataReceiver<T>),
    Datagram(DatagramReceiver<T>),
}

#[derive(Debug)]
pub struct Subscription<T: TransportProtocol> {
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub delivery_timeout: Option<u64>,
    pub(crate) receiver: Option<tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>>,
}

impl<T: TransportProtocol> Subscription<T> {
    pub(crate) async fn new(subscribe_ok: SubscribeOk, session: &Arc<SessionContext<T>>) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<StreamWithObject<T>>();
        session
            .notification_map
            .write()
            .await
            .insert(subscribe_ok.track_alias, sender);
        Self {
            track_alias: subscribe_ok.track_alias,
            expires: subscribe_ok.expires,
            group_order: subscribe_ok.group_order,
            content_exists: subscribe_ok.content_exists,
            delivery_timeout: None,
            receiver: Some(receiver),
        }
    }
}
