use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use client_telemetry::{
    SpanRecordLayer, continue_trace_from_traceparent, current_traceparent, log_record, now_ms,
    random_u64,
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde_json::json;
use tracing::{Event, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

/// How long `wait_log_records` waits for a first record before it returns an
/// empty list, so the thread that calls it sees its stop flag at exit.
const WAIT: Duration = Duration::from_millis(500);
/// The most records one `wait_log_records` returns.
const MAX_BATCH: usize = 256;
/// The records the queue holds while the sink thread waits for the GIL: 16
/// full batches. Records beyond it are dropped and counted, so a Python thread
/// holding the GIL cannot make the module's logging grow without bound.
const QUEUE_CAPACITY: usize = 16 * MAX_BATCH;

/// A log or span record, as the JSON of the observability server's
/// `/api/client-telemetry`.
struct LogSinkRecord {
    kind: &'static str,
    json: String,
}

static RECORDS: OnceLock<Mutex<mpsc::Receiver<LogSinkRecord>>> = OnceLock::new();
/// Records dropped because the queue was full, since the last `wait_log_records`.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Installs the process-wide `tracing` subscriber that queues every event at
/// `level` (`error`, `warn`, `info`, `debug` or `trace`) or above as a log
/// record, and, with `spans`, every closed INFO or higher span as a span
/// record, which then carries the trace of the requests that send
/// TRACE_CONTEXT. `wait_log_records` takes them. Fails when the level is
/// unknown or a subscriber is already set: it can be installed once.
#[pyfunction]
pub fn set_log_sink(level: &str, spans: bool) -> PyResult<()> {
    let level: LevelFilter = level
        .parse()
        .map_err(|_| PyValueError::new_err(format!("unknown log level: {level}")))?;
    let (record_sender, record_receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    let span_sender = record_sender.clone();
    let span_layer = spans.then(|| {
        SpanRecordLayer {
            now_ms,
            random_byte: || random_u64() as u8,
            emit: move |record: serde_json::Value, _scope: Option<u64>| {
                queue(&span_sender, "span", record.to_string());
            },
        }
        .with_filter(LevelFilter::INFO)
    });
    let subscriber = tracing_subscriber::registry()
        .with(LogLayer(record_sender).with_filter(level))
        .with(span_layer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| PyRuntimeError::new_err("the log sink is already set"))?;
    let _ = RECORDS.set(Mutex::new(record_receiver));
    if spans {
        moqt::set_trace_context_provider(current_traceparent);
        moqt::set_trace_context_joiner(continue_trace_from_traceparent);
    }
    Ok(())
}

/// The records queued since the last call, as `(kind, json)` with `kind`
/// `log` or `span`, led by a `log records dropped` warning when the queue was
/// full meanwhile; waits half a second for the first one, without holding
/// the GIL, and returns an empty list after that. Fails before
/// `set_log_sink`.
#[pyfunction]
pub fn wait_log_records(py: Python<'_>) -> PyResult<Vec<(&'static str, String)>> {
    let records = RECORDS
        .get()
        .ok_or_else(|| PyRuntimeError::new_err("call set_log_sink first"))?;
    Ok(py.detach(|| {
        let records = records.lock().unwrap();
        let Ok(first) = records.recv_timeout(WAIT) else {
            return Vec::new();
        };
        let dropped = DROPPED.swap(0, Ordering::Relaxed);
        let dropped = (dropped > 0).then(|| LogSinkRecord {
            kind: "log",
            json: json!({
                "timeMs": now_ms(),
                "level": "warn",
                "target": "moq_wasm::log_sink",
                "message": "log records dropped",
                "fields": { "count": dropped },
            })
            .to_string(),
        });
        dropped
            .into_iter()
            .chain(std::iter::once(first))
            .chain(records.try_iter().take(MAX_BATCH - 1))
            .map(|record| (record.kind, record.json))
            .collect()
    }))
}

fn queue(sender: &mpsc::SyncSender<LogSinkRecord>, kind: &'static str, json: String) {
    if sender.try_send(LogSinkRecord { kind, json }).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

struct LogLayer(mpsc::SyncSender<LogSinkRecord>);

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        queue(&self.0, "log", log_record(event, now_ms()).to_string());
    }
}
