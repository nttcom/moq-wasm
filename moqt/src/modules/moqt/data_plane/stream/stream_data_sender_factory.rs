use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            data_plane::stream::stream_data_sender::StreamDataSender,
            domains::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct StreamDataSenderFactory<T: TransportProtocol> {
    track_alias: u64,
    subscriber_priority: u8,
    session: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> StreamDataSenderFactory<T> {
    pub(crate) fn new(
        track_alias: u64,
        subscriber_priority: u8,
        session: Arc<SessionContext<T>>,
    ) -> Self {
        Self {
            track_alias,
            subscriber_priority,
            session,
        }
    }

    pub async fn next(&self) -> anyhow::Result<StreamDataSender<T>> {
        let send_stream = self.session.transport_connection.open_uni().await?;
        Ok(StreamDataSender::new(
            self.track_alias,
            self.subscriber_priority,
            send_stream,
        ))
    }
}
