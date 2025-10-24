use bytes::BytesMut;

pub(crate) trait DatagramSender {
    fn create_object_datagram_from_binary(
        &self,
        binary_object: BytesMut,
    ) -> anyhow::Result<moqt::DatagramObject>;
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()>;
}

impl<T: moqt::TransportProtocol> DatagramSender for moqt::DatagramSender<T> {
    fn create_object_datagram_from_binary(
        &self,
        binary_object: BytesMut,
    ) -> anyhow::Result<moqt::DatagramObject> {
        self.create_object_datagram_from_binary(binary_object)
    }
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()> {
        self.send(object)
    }
}
