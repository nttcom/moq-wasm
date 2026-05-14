use crate::{
    GroupOrder,
    modules::moqt::control_plane::control_messages::messages::{
        parameters::content_exists::ContentExists, subscribe_ok::SubscribeOk,
    },
};

#[derive(Debug)]
pub struct Subscription {
    pub request_id: u64,
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub delivery_timeout: Option<u64>,
}

impl Subscription {
    pub(crate) fn new(subscribe_ok: SubscribeOk) -> Self {
        Self {
            request_id: subscribe_ok.request_id,
            track_alias: subscribe_ok.track_alias,
            expires: subscribe_ok.expires,
            group_order: subscribe_ok.group_order,
            content_exists: subscribe_ok.content_exists,
            delivery_timeout: None,
        }
    }
}
