use std::sync::Arc;

use crate::{
    Session,
    modules::{
        client::routes::Routes,
        executor::{self, JoinHandle},
    },
};

/// Reads the session's events into `Routes` until the session ends, `stop` is
/// called or the task is dropped.
pub(super) struct EventLoopTask {
    handle: JoinHandle,
}

impl EventLoopTask {
    pub(super) fn run(session: Arc<Session>, routes: Arc<Routes>) -> Self {
        let handle = executor::spawn("moqt-client-session-events", async move {
            routes.route(&session).await;
        });
        Self { handle }
    }

    pub(super) fn stop(&self) {
        self.handle.abort();
    }
}

impl Drop for EventLoopTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
