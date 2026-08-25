pub(crate) trait FetchCancelHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
}

impl FetchCancelHandler for moqt::FetchCancelHandler {
    fn request_id(&self) -> u64 {
        self.request_id()
    }
}
