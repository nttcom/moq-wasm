use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

use telemetry::client_report::{ClientLog, ClientSpan, MAX_RECORDS};

#[derive(Default)]
struct Records {
    logs: VecDeque<ClientLog>,
    spans: VecDeque<ClientSpan>,
}

/// The newest `MAX_RECORDS` logs and closed spans since the last report.
#[derive(Clone, Default)]
pub struct ReportBuffer(Arc<Mutex<Records>>);

impl ReportBuffer {
    pub(crate) fn push_log(&self, log: ClientLog) {
        keep_newest(&mut self.lock().logs, log);
    }

    pub(crate) fn push_span(&self, span: ClientSpan) {
        keep_newest(&mut self.lock().spans, span);
    }

    /// The logs and the spans, emptying the buffer.
    pub fn take(&self) -> (Vec<ClientLog>, Vec<ClientSpan>) {
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

fn keep_newest<T>(records: &mut VecDeque<T>, record: T) {
    if records.len() == MAX_RECORDS {
        records.pop_front();
    }
    records.push_back(record);
}

#[cfg(test)]
mod tests {
    use telemetry::client_report::ClientLog;

    use super::{MAX_RECORDS, ReportBuffer};

    fn log(time_ms: u64) -> ClientLog {
        ClientLog {
            time_ms,
            level: "info".into(),
            target: "moqt".into(),
            message: String::new(),
            fields: Default::default(),
            trace_id: None,
            span_id: None,
        }
    }

    #[test]
    fn a_full_buffer_keeps_the_newest_logs() {
        // Arrange
        let buffer = ReportBuffer::default();

        // Act
        for index in 0..=MAX_RECORDS {
            buffer.push_log(log(index as u64));
        }

        // Assert
        let (logs, spans) = buffer.take();
        assert_eq!(logs.len(), MAX_RECORDS);
        assert_eq!(logs[0].time_ms, 1);
        assert!(spans.is_empty());
        assert!(buffer.take().0.is_empty());
    }
}
