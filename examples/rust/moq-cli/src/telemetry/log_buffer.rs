use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use serde_json::Value;

/// The observability server takes at most this many logs per report.
const MAX_LOGS: usize = 100;

/// The logs collected since the last report, the newest `MAX_LOGS` of them.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<VecDeque<Value>>>);

impl LogBuffer {
    pub(super) fn push(&self, log: Value) {
        let mut logs = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if logs.len() == MAX_LOGS {
            logs.pop_front();
        }
        logs.push_back(log);
    }

    pub(super) fn take(&self) -> Vec<Value> {
        let mut logs = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        logs.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{LogBuffer, MAX_LOGS};

    #[test]
    fn a_full_buffer_keeps_the_newest_logs() {
        // Arrange
        let buffer = LogBuffer::default();

        // Act
        for index in 0..=MAX_LOGS {
            buffer.push(json!(index));
        }

        // Assert
        let logs = buffer.take();
        assert_eq!(logs.len(), MAX_LOGS);
        assert_eq!(logs[0], json!(1));
        assert!(buffer.take().is_empty());
    }
}
