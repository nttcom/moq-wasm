use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            messages::object::datagram_object::DatagramObject,
            sessions::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramReceiver<T: TransportProtocol> {
    pub(crate) session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramReceiver<T> {
    pub async fn receive(&self) -> anyhow::Result<DatagramObject> {
        let mut bytes = self
            .session_context
            .transport_connection
            .receive_datagram()
            .await?;
        DatagramObject::depacketize(&mut bytes)
    }
}
