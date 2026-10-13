use std::sync::Arc;

use crate::modules::{
    executor,
    moqt::{
        data_plane::object::{canonical_object::Object, object_datagram::ObjectDatagram},
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

    /// Sends `object` as one datagram of this track; its forwarding preference
    /// is not read.
    pub async fn send_object(&mut self, object: &Object) -> anyhow::Result<()> {
        let bytes = ObjectDatagram::from_object(self.track_alias, object).encode()?;
        let result = self
            .session_context
            .transport_connection
            .send_datagram(bytes);
        executor::yield_now().await;
        result
    }
}
