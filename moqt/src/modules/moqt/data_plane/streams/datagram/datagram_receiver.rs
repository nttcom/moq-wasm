use std::sync::Arc;

use anyhow::bail;

use crate::modules::moqt::data_plane::object::object_datagram::ObjectDatagram;
use crate::{TransportProtocol, modules::moqt::control_plane::models::session_context::SessionContext};

#[derive(Debug)]
pub struct DatagramReceiver {
    pub track_alias: u64,
    receiver: tokio::sync::mpsc::UnboundedReceiver<ObjectDatagram>,
}

impl DatagramReceiver {
    pub(crate) async fn new<T: TransportProtocol>(
        session_context: Arc<SessionContext<T>>,
        track_alias: u64,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<ObjectDatagram>();
        session_context
            .datagram_sender_map
            .write()
            .await
            .insert(track_alias, sender);
        Self {
            track_alias,
            receiver,
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<ObjectDatagram> {
        match self.receiver.recv().await {
            Some(object) => Ok(object),
            None => bail!("Sender has been dropped."),
        }
    }
}
