pub(crate) trait UnsubscribeNamespaceHandler: 'static + Send + Sync {
    fn track_namespace_prefix(&self) -> &str;
}

impl UnsubscribeNamespaceHandler for moqt::UnsubscribeNamespaceHandler {
    fn track_namespace_prefix(&self) -> &str {
        self.track_namespace_prefix()
    }
}
