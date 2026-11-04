use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

use serde_json::Value;

/// The observability server takes at most this many logs, and as many spans, per report.
const MAX_RECORDS: usize = 100;

#[derive(Default)]
struct Records {
    logs: VecDeque<Value>,
    spans: VecDeque<Value>,
}

/// The newest `MAX_RECORDS` logs and closed spans since the last report.
#[derive(Clone, Default)]
pub struct ReportBuffer(Arc<Mutex<Records>>);

impl ReportBuffer {
    pub(crate) fn push_log(&self, log: Value) {
        keep_newest(&mut self.lock().logs, log);
    }

    pub(crate) fn push_span(&self, span: Value) {
        keep_newest(&mut self.lock().spans, span);
    }

    /// The logs and the spans, emptying the buffer.
    pub fn take(&self) -> (Vec<Value>, Vec<Value>) {
        let mut records = self.lock();
        (
            records.logs.drain(..).collect(),
            records.spans.drain(..).collect(),
        )
    }

    fn lock(&self) -> MutexGuard<'_, Records> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn keep_newest(records: &mut VecDeque<Value>, record: Value) {
    if records.len() == MAX_RECORDS {
        records.pop_front();
    }
    records.push_back(record);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MAX_RECORDS, ReportBuffer};

    #[test]
    fn a_full_buffer_keeps_the_newest_logs() {
        // Arrange
        let buffer = ReportBuffer::default();

        // Act
        for index in 0..=MAX_RECORDS {
            buffer.push_log(json!(index));
        }

        // Assert
        let (logs, spans) = buffer.take();
        assert_eq!(logs.len(), MAX_RECORDS);
        assert_eq!(logs[0], json!(1));
        assert!(spans.is_empty());
        assert!(buffer.take().0.is_empty());
    }
}
