use std::sync::Arc;

use crate::{
    DatagramReceiver, GroupOrder, StreamDataReceiver, TransportProtocol,
    modules::moqt::control_plane::{
        messages::control_messages::{enums::ContentExists, subscribe_ok::SubscribeOk},
        models::session_context::SessionContext,
        threads::enums::StreamWithObject,
    },
};

pub enum DataReceiver<T: TransportProtocol> {
    Stream(StreamDataReceiver<T>),
    Datagram(DatagramReceiver<T>),
}

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
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<StreamWithObject<T>>();
        self.session_context
            .notification_map
            .write()
            .await
            .insert(self.track_alias, sender);
        let result = receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))?;
        match result {
            StreamWithObject::StreamHeader { stream, header } => {
                let data_receiver = StreamDataReceiver::new(receiver, stream, header).await?;
                Ok(DataReceiver::Stream(data_receiver))
            }
            StreamWithObject::Datagram(object) => {
                let data_receiver = DatagramReceiver::new(object, receiver).await;
                Ok(DataReceiver::Datagram(data_receiver))
            }
        }
    }
}
