pub(crate) trait GoAwayHandler: 'static + Send + Sync {
    fn new_session_uri(&self) -> &str;
}

impl GoAwayHandler for moqt::GoAwayHandler {
    fn new_session_uri(&self) -> &str {
        self.new_session_uri()
    }
}
