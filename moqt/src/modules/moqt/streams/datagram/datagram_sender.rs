use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::{
        moqt::sessions::session_context::SessionContext,
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramSender<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramSender<T> {
    fn send(&self, bytes: BytesMut) {
        self.session_context
            .transport_connection
            .send_datagram(bytes)
            .unwrap();
    }
}
