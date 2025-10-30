use std::sync::Arc;

use anyhow::bail;

use crate::{
    DatagramObject, DatagramReceiver, GroupOrder, TransportProtocol,
    modules::{
        moqt::{
            messages::control_messages::{location::Location, subscribe_ok::SubscribeOk},
            sessions::session_context::SessionContext,
            streams::stream::stream_receiver::StreamReceiver,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub enum Acceptance<T: TransportProtocol> {
    Stream(StreamReceiver<T>),
    Datagram(DatagramReceiver, DatagramObject),
}

pub struct Subscription<T: TransportProtocol> {
    pub(crate) session_context: Arc<SessionContext<T>>,
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: bool,
    pub largest_location: Option<Location>,
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
            largest_location: subscribe_ok.largest_location,
            derivery_timeout: None,
        }
    }

    pub async fn accept_stream_or_datagram(
        &self,
        track_alias: u64,
    ) -> anyhow::Result<Acceptance<T>> {
        let mut datagram_receiver =
            DatagramReceiver::new(self.session_context.clone(), track_alias).await;

        tokio::select! {
            stream = self.session_context.transport_connection.accept_uni() => {
                Ok(Acceptance::Stream(StreamReceiver::new(stream?)))
            }
            datagram = datagram_receiver.receive() => {
                if let Ok(datagram) = datagram {
                    Ok(Acceptance::Datagram(datagram_receiver, datagram))
                } else {
                    bail!("Failed to receive datagram")
                }
            }
        }
    }
}
