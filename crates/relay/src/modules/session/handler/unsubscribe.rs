pub(crate) trait UnsubscribeHandler: 'static + Send + Sync {
    fn subscribe_id(&self) -> u64;
}

impl UnsubscribeHandler for moqt::UnsubscribeHandler {
    fn subscribe_id(&self) -> u64 {
        self.subscribe_id()
    }
}
