use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            data_plane::object::object_datagram::ObjectDatagram,
            domains::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramSender<T: TransportProtocol> {
    pub track_alias: u64,
    session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramSender<T> {
    pub(crate) fn new(track_alias: u64, session_context: Arc<SessionContext<T>>) -> Self {
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
        tokio::task::yield_now().await;
        result
    }
}
