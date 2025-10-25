pub(crate) trait DatagramSender: 'static + Send + Sync {
    fn overwrite_track_alias_then_send(&self, object: moqt::DatagramObject) -> anyhow::Result<()>;
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()>;
}

impl<T: moqt::TransportProtocol> DatagramSender for moqt::DatagramSender<T> {
    fn overwrite_track_alias_then_send(&self, object: moqt::DatagramObject) -> anyhow::Result<()> {
        self.overwrite_track_alias_then_send(object)
    }
    fn send(&self, object: moqt::DatagramObject) -> anyhow::Result<()> {
        self.send(object)
    }
}
