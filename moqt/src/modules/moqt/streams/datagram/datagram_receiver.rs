use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::{
        moqt::sessions::session_context::SessionContext,
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramReceiver<T: TransportProtocol> {
    pub(crate) session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramReceiver<T> {
    pub async fn receive(&self) -> anyhow::Result<BytesMut> {
        self.session_context
            .transport_connection
            .receive_datagram()
            .await
    }
}
