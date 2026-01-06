use std::sync::Arc;

use crate::{
    GroupOrder, TransportProtocol,
    modules::moqt::{
        control_plane::{
            messages::control_messages::{enums::ContentExists, subscribe_ok::SubscribeOk},
            models::session_context::SessionContext,
        },
        data_plane::streams::data_receiver::DataReceiver,
    },
};

pub struct Subscription<T: TransportProtocol> {
    pub(crate) session_context: Arc<SessionContext<T>>,
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub derivery_timeout: Option<u64>,
}

impl<T: TransportProtocol> Subscription<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, subscribe_ok: SubscribeOk) -> Self {
        Self {
            session_context,
            track_alias: subscribe_ok.track_alias,
            expires: subscribe_ok.expires,
            group_order: subscribe_ok.group_order,
            content_exists: subscribe_ok.content_exists,
            derivery_timeout: None,
        }
    }

    pub async fn accept_data_receiver(&self) -> anyhow::Result<DataReceiver<T>> {
        DataReceiver::new(&self.session_context, self.track_alias).await
    }
}
