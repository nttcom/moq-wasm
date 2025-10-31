pub(crate) trait DatagramSender: 'static + Send + Sync {
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()>;
}

impl<T: moqt::TransportProtocol> DatagramSender for moqt::DatagramSender<T> {
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()> {
        self.send(object)
    }
}
