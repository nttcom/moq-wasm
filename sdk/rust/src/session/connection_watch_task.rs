use std::sync::Arc;

use moqt::{
    client,
    executor::{self, JoinHandle},
};
use tokio::sync::watch;

use super::{SessionEnd, end};

/// Ends the session with `ConnectionLost` once its last connection closed.
pub(super) struct ConnectionWatchTask {
    join_handle: JoinHandle,
}

impl ConnectionWatchTask {
    pub(super) fn run(
        client: Arc<client::Session>,
        end_sender: Arc<watch::Sender<Option<SessionEnd>>>,
    ) -> Self {
        let join_handle = executor::spawn("moq-wasm-connection-watch", async move {
            let close = client.closed().await;
            end(&end_sender, SessionEnd::ConnectionLost(close));
        });
        Self { join_handle }
    }
}

impl Drop for ConnectionWatchTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
