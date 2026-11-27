use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use telemetry::client_report::{ClientLog, ClientSpan, MAX_RECORDS};

/// Most tokens with queued records; a record of a new token past this evicts
/// the batch queued first.
const MAX_QUEUED_TOKENS: usize = 64;

/// The newest `MAX_RECORDS` logs and spans of one token (`token` is `None`
/// outside any session).
pub(super) struct Batch {
    pub(super) token: Option<String>,
    pub(super) logs: VecDeque<ClientLog>,
    pub(super) spans: VecDeque<ClientSpan>,
}

#[derive(Default)]
struct Queued {
    /// Oldest token first.
    batches: Vec<Batch>,
    /// Records evicted with their batch since the last `take`.
    dropped: usize,
}

/// The logs and spans queued for the next report, per token.
#[derive(Clone, Default)]
pub(super) struct ReportBatches(Arc<Mutex<Queued>>);

impl ReportBatches {
    pub(super) fn push_log(&self, token: Option<String>, log: ClientLog) {
        self.with_batch(token, |batch| keep_newest(&mut batch.logs, log));
    }

    pub(super) fn push_span(&self, token: Option<String>, span: ClientSpan) {
        self.with_batch(token, |batch| keep_newest(&mut batch.spans, span));
    }

    /// The batches and the count of records evicted meanwhile, emptying the queue.
    pub(super) fn take(&self) -> (Vec<Batch>, usize) {
        let mut queued = self.0.lock().unwrap();
        let dropped = std::mem::take(&mut queued.dropped);
        (std::mem::take(&mut queued.batches), dropped)
    }

    fn with_batch(&self, token: Option<String>, push: impl FnOnce(&mut Batch)) {
        let mut queued = self.0.lock().unwrap();
        let index = match queued.batches.iter().position(|batch| batch.token == token) {
            Some(index) => index,
            None => {
                if queued.batches.len() == MAX_QUEUED_TOKENS {
                    let evicted = queued.batches.remove(0);
                    queued.dropped += evicted.logs.len() + evicted.spans.len();
                }
                queued.batches.push(Batch {
                    token,
                    logs: VecDeque::new(),
                    spans: VecDeque::new(),
                });
                queued.batches.len() - 1
            }
        };
        push(&mut queued.batches[index]);
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
    use super::*;

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
    fn a_full_batch_keeps_the_newest_logs_of_its_token() {
        // Arrange
        let batches = ReportBatches::default();

        // Act
        for index in 0..=MAX_RECORDS {
            batches.push_log(Some("a".into()), log(index as u64));
        }
        batches.push_log(None, log(0));

        // Assert
        let (taken, dropped) = batches.take();
        let [a, untokenized] = taken.as_slice() else {
            panic!("{} batches", taken.len());
        };
        assert_eq!(a.token.as_deref(), Some("a"));
        assert_eq!(a.logs.len(), MAX_RECORDS);
        assert_eq!(a.logs[0].time_ms, 1);
        assert_eq!(
            (untokenized.token.as_deref(), untokenized.logs.len()),
            (None, 1)
        );
        assert_eq!(dropped, 0);
        assert!(batches.take().0.is_empty());
    }

    #[test]
    fn a_new_token_past_the_limit_evicts_the_oldest_batch() {
        // Arrange
        let batches = ReportBatches::default();
        for index in 0..MAX_QUEUED_TOKENS {
            batches.push_log(Some(index.to_string()), log(0));
        }

        // Act
        batches.push_log(Some("new".into()), log(0));

        // Assert
        let (taken, dropped) = batches.take();
        assert_eq!(taken.len(), MAX_QUEUED_TOKENS);
        assert_eq!(taken[0].token.as_deref(), Some("1"));
        assert_eq!(taken[MAX_QUEUED_TOKENS - 1].token.as_deref(), Some("new"));
        assert_eq!(dropped, 1);
    }
}
