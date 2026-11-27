use std::{sync::Arc, time::Duration};

use moqt::executor::{self, AbortOnDrop};

use super::reporter::Reporter;

const FLUSH_INTERVAL: Duration = Duration::from_secs(10);

pub(super) struct FlushTask {
    _join_handle: AbortOnDrop,
}

impl FlushTask {
    pub(super) fn run(reporter: Arc<Reporter>) -> Self {
        let join_handle = executor::spawn("moq-wasm-telemetry-flush", async move {
            loop {
                tokio::time::sleep(FLUSH_INTERVAL).await;
                reporter.flush().await;
            }
        })
        .abort_on_drop();
        Self {
            _join_handle: join_handle,
        }
    }
}
