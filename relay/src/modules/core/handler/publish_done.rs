pub(crate) trait PublishDoneHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn status_code(&self) -> u64;
    fn stream_count(&self) -> u64;
    fn error_reason(&self) -> &str;
}

impl PublishDoneHandler for moqt::PublishDoneHandler {
    fn request_id(&self) -> u64 {
        self.request_id()
    }

    fn status_code(&self) -> u64 {
        self.status_code()
    }

    fn stream_count(&self) -> u64 {
        self.stream_count()
    }

    fn error_reason(&self) -> &str {
        self.error_reason()
    }
}
