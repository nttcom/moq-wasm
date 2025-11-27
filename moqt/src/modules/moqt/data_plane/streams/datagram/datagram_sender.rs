use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::control_plane::models::session_context::SessionContext,
        moqt::data_plane::object::object_datagram::ObjectDatagram,
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramHeader {
    pub group_id: u64,
    pub object_id: Option<u64>,
    pub publisher_priority: u8,
    pub prior_object_id_gap: Option<u64>,
    pub prior_group_id_gap: Option<u64>,
    pub immutable_extensions: Vec<u8>,
}

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

    pub async fn send(&self, object: ObjectDatagram) -> anyhow::Result<()> {
        let bytes = object.encode();
        let result = self
            .session_context
            .transport_connection
            .send_datagram(bytes);
        tokio::task::yield_now().await;
        result
    }
}
