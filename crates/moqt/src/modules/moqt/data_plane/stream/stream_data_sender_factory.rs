use std::sync::Arc;

use crate::modules::moqt::{
    data_plane::stream::stream_data_sender::StreamDataSender,
    domains::session_context::SessionContext,
};

pub struct StreamDataSenderFactory {
    track_alias: u64,
    session: Arc<SessionContext>,
}

impl StreamDataSenderFactory {
    pub(crate) fn new(track_alias: u64, session: Arc<SessionContext>) -> Self {
        Self {
            track_alias,
            session,
        }
    }

    pub async fn next(&self) -> anyhow::Result<StreamDataSender> {
        let send_stream = self.session.transport_connection.open_uni().await?;
        Ok(StreamDataSender::new(self.track_alias, send_stream))
    }
}
