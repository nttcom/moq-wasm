use std::sync::Arc;

use moqt::{
    client,
    executor::{self, AbortOnDrop},
};
use tokio::sync::watch;

use super::{SessionEnd, end};

/// Ends the session with `ConnectionLost` once its last connection closed.
pub(super) struct ConnectionWatchTask {
    _join_handle: AbortOnDrop,
}

impl ConnectionWatchTask {
    pub(super) fn run(
        client: Arc<client::Session>,
        end_sender: Arc<watch::Sender<Option<SessionEnd>>>,
    ) -> Self {
        let join_handle = executor::spawn("moq-wasm-connection-watch", async move {
            let close = client.closed().await;
            end(&end_sender, SessionEnd::ConnectionLost(close));
        })
        .abort_on_drop();
        Self {
            _join_handle: join_handle,
        }
    }
}
