pub(crate) trait PublishNamespaceCancelHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
    fn error_code(&self) -> u64;
    fn error_reason(&self) -> &str;
}

impl PublishNamespaceCancelHandler for moqt::PublishNamespaceCancelHandler {
    fn track_namespace(&self) -> &str {
        self.track_namespace()
    }

    fn error_code(&self) -> u64 {
        self.error_code()
    }

    fn error_reason(&self) -> &str {
        self.error_reason()
    }
}
