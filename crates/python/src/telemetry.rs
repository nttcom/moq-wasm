use std::sync::OnceLock;

use client_telemetry::{ReportBuffer, current_traceparent, report_layer};
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use serde_json::json;
use tracing_subscriber::{Layer, filter::LevelFilter, layer::SubscriberExt};

static BUFFER: OnceLock<ReportBuffer> = OnceLock::new();

/// Collects the module's INFO and higher logs and spans for `take_telemetry`,
/// and sends a TRACE_CONTEXT with each request so the relay continues its
/// trace. Calls after a successful one do nothing. Fails when the process
/// already has a global `tracing` subscriber.
#[pyfunction]
pub fn start_telemetry() -> PyResult<()> {
    if BUFFER.get().is_some() {
        return Ok(());
    }
    let buffer = ReportBuffer::default();
    let subscriber = tracing_subscriber::registry()
        .with(report_layer(buffer.clone()).with_filter(LevelFilter::INFO));
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| PyRuntimeError::new_err("a tracing subscriber is already set"))?;
    moqt::set_trace_context_provider(current_traceparent);
    // The GIL serializes calls, so no other call set the buffer since the check above.
    let _ = BUFFER.set(buffer);
    Ok(())
}

/// The logs and spans collected since the last call, as
/// `{"logs": [...], "spans": [...]}` in the records of the observability
/// server's `/api/client-telemetry`; empty before `start_telemetry`.
#[pyfunction]
pub fn take_telemetry(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let (logs, spans) = BUFFER.get().map(ReportBuffer::take).unwrap_or_default();
    let records = json!({ "logs": logs, "spans": spans });
    py.import("json")?
        .call_method1("loads", (records.to_string(),))
}
