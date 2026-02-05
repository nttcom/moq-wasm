use std::sync::Arc;

use crate::{
    DatagramField, TransportProtocol,
    modules::{
        moqt::{
            control_plane::models::session_context::SessionContext,
            data_plane::object::object_datagram::ObjectDatagram,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramSender<T: TransportProtocol> {
    pub track_alias: u64,
    pub end_of_group: bool,
    session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramSender<T> {
    pub(crate) fn new(track_alias: u64, session_context: Arc<SessionContext<T>>) -> Self {
        Self {
            track_alias,
            end_of_group: false,
            session_context,
        }
    }

    pub fn create_object_datagram(&self, group_id: u64, data: DatagramField) -> ObjectDatagram {
        ObjectDatagram::new(self.track_alias, group_id, data)
    }

    pub async fn send(&mut self, data: ObjectDatagram) -> anyhow::Result<()> {
        let bytes = data.encode();
        let result = self
            .session_context
            .transport_connection
            .send_datagram(bytes);
        tokio::task::yield_now().await;
        result
    }
}
