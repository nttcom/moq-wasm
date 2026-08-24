pub(crate) trait TrackStatusHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn track_namespace(&self) -> &str;
    fn track_name(&self) -> &str;
}

impl<T: moqt::TransportProtocol> TrackStatusHandler for moqt::TrackStatusHandler<T> {
    fn request_id(&self) -> u64 {
        self.request_id()
    }

    fn track_namespace(&self) -> &str {
        self.track_namespace()
    }

    fn track_name(&self) -> &str {
        self.track_name()
    }
}
