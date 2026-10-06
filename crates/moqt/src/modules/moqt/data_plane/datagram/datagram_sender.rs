use std::sync::Arc;

use crate::modules::{
    executor,
    moqt::{
        data_plane::object::object_datagram::ObjectDatagram,
        domains::session_context::SessionContext,
    },
};

pub struct DatagramSender {
    pub track_alias: u64,
    session_context: Arc<SessionContext>,
}

impl DatagramSender {
    pub(crate) fn new(track_alias: u64, session_context: Arc<SessionContext>) -> Self {
        Self {
            track_alias,
            session_context,
        }
    }

    pub async fn send(&mut self, data: ObjectDatagram) -> anyhow::Result<()> {
        let bytes = data.encode()?;
        let result = self
            .session_context
            .transport_connection
            .send_datagram(bytes);
        executor::yield_now().await;
        result
    }
}
