pub(crate) trait UnsubscribeHandler: 'static + Send + Sync {
    fn subscribe_id(&self) -> u64;
}

impl<T: moqt::TransportProtocol> UnsubscribeHandler for moqt::UnsubscribeHandler<T> {
    fn subscribe_id(&self) -> u64 {
        self.subscribe_id()
    }
}
