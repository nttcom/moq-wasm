use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            control_plane::models::session_context::SessionContext,
            data_plane::{object::data_object::DataObject, streams::stream_type::SendStreamType},
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramSender<T: TransportProtocol> {
    pub track_alias: u64,
    pub end_of_group: bool,
    session_context: Arc<SessionContext<T>>,
}

#[async_trait::async_trait]
impl<T: TransportProtocol> SendStreamType for DatagramSender<T> {
    fn is_datagram(&self) -> bool {
        true
    }

    async fn send(&mut self, data: DataObject) -> anyhow::Result<()> {
        match data {
            DataObject::ObjectDatagram(object) => {
                let bytes = object.encode();
                let result = self
                    .session_context
                    .transport_connection
                    .send_datagram(bytes);
                tokio::task::yield_now().await;
                result
            }
            _ => unreachable!("DatagramSender can only send ObjectDatagram"),
        }
    }
}

impl<T: TransportProtocol> DatagramSender<T> {
    pub(crate) fn new(track_alias: u64, session_context: Arc<SessionContext<T>>) -> Self {
        Self {
            track_alias,
            end_of_group: false,
            session_context,
        }
    }
}
