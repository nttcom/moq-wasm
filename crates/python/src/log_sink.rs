use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, LazyLock, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde_json::json;
use telemetry_report::{
    ClientSpan, SpanRecordLayer, continue_trace_from_traceparent, current_scope,
    current_traceparent, log_record, now_ms, random_u64, scoped,
};
use tracing::{Event, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

use crate::scope::SessionScope;

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
/// `/api/client-telemetry`, with the token of the session it belongs to.
struct LogSinkRecord {
    kind: &'static str,
    json: String,
    token: Option<String>,
}

/// The token of each session scope that has one.
static SCOPE_TOKENS: LazyLock<Mutex<HashMap<u64, Weak<Mutex<String>>>>> =
    LazyLock::new(Mutex::default);
static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

/// A new scope for a session, whose records then carry `token`'s value at
/// the time they are emitted.
pub(crate) fn session_scope(token: Option<&Arc<Mutex<String>>>) -> SessionScope {
    let scope = NEXT_SCOPE.fetch_add(1, Ordering::Relaxed);
    if let Some(token) = token {
        let mut tokens = SCOPE_TOKENS.lock().unwrap();
        tokens.retain(|_, token| token.strong_count() > 0);
        tokens.insert(scope, Arc::downgrade(token));
    }
    SessionScope(scope)
}

fn token_of(scope: Option<u64>) -> Option<String> {
    let token = SCOPE_TOKENS.lock().unwrap().get(&scope?)?.upgrade()?;
    let token = token.lock().unwrap().clone();
    Some(token)
}

type SpawnedTask = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Polls each task `moqt::executor::spawn` starts in the scope it was spawned
/// in, so the session's tasks keep its token.
fn carry_scope(future: SpawnedTask) -> SpawnedTask {
    match current_scope() {
        Some(scope) => Box::pin(scoped(scope, future)),
        None => future,
    }
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
            emit: move |record: ClientSpan, scope: Option<u64>| {
                queue(&span_sender, "span", record.to_json(), token_of(scope));
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
    moqt::executor::set_spawn_wrapper(carry_scope);
    if spans {
        moqt::set_trace_context_provider(current_traceparent);
        moqt::set_trace_context_joiner(continue_trace_from_traceparent);
    }
    Ok(())
}

/// The records queued since the last call, as `(kind, json, token)` with
/// `kind` `log` or `span` and the token of the session the record belongs to,
/// led by a `log records dropped` warning when the queue was
/// full meanwhile; waits half a second for the first one, without holding
/// the GIL, and returns an empty list after that. Fails before
/// `set_log_sink`.
#[pyfunction]
pub fn wait_log_records(py: Python<'_>) -> PyResult<Vec<(&'static str, String, Option<String>)>> {
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
            token: None,
        });
        dropped
            .into_iter()
            .chain(std::iter::once(first))
            .chain(records.try_iter().take(MAX_BATCH - 1))
            .map(|record| (record.kind, record.json, record.token))
            .collect()
    }))
}

fn queue(
    sender: &mpsc::SyncSender<LogSinkRecord>,
    kind: &'static str,
    json: String,
    token: Option<String>,
) {
    if sender
        .try_send(LogSinkRecord { kind, json, token })
        .is_err()
    {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

struct LogLayer(mpsc::SyncSender<LogSinkRecord>);

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let token = token_of(current_scope());
        queue(&self.0, "log", log_record(event, now_ms()).to_json(), token);
    }
}
