pub(crate) trait StreamSender: 'static + Send + Sync {
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()>;
}

impl<T: moqt::TransportProtocol> StreamSender for moqt::DatagramSender<T> {
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()> {
        todo!()
    }
}
