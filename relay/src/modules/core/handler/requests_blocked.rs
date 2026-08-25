pub(crate) trait RequestsBlockedHandler: 'static + Send + Sync {
    fn maximum_request_id(&self) -> u64;
}

impl RequestsBlockedHandler for moqt::RequestsBlockedHandler {
    fn maximum_request_id(&self) -> u64 {
        self.maximum_request_id()
    }
}
