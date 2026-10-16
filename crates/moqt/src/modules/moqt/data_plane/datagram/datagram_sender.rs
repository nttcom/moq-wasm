use std::sync::Arc;

use crate::modules::{
    executor,
    moqt::{
        data_plane::object::canonical_object::Object, domains::session_context::SessionContext,
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
        let bytes = self
            .session_context
            .connection()
            .control
            .version()
            .encode_datagram(self.track_alias, object)?;
        let result = self
            .session_context
            .connection()
            .transport
            .send_datagram(bytes);
        executor::yield_now().await;
        result
    }
}
