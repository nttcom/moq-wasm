pub(crate) trait PublishNamespaceDoneHandler: 'static + Send + Sync {
    fn track_namespace(&self) -> &str;
}

impl PublishNamespaceDoneHandler for moqt::PublishNamespaceDoneHandler {
    fn track_namespace(&self) -> &str {
        self.track_namespace()
    }
}
