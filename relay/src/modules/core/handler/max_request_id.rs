pub(crate) trait MaxRequestIdHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
}

impl MaxRequestIdHandler for moqt::MaxRequestIdHandler {
    fn request_id(&self) -> u64 {
        self.request_id()
    }
}
