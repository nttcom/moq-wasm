# telemetry-report

Lightweight client-side `tracing` collector that converts events and closed spans into [`telemetry::client_report`](../telemetry/README.md) (`ClientLog` and `ClientSpan`) records and propagates W3C `traceparent` headers without pulling in the OpenTelemetry SDK.

> **Workspace crate**: Internal library used by `sdk/js` (wasm32), `sdk/python`, and `examples/rust/moq-cli`. Compiles on native targets and `wasm32`.

## Usage

### Native clients (`report_layer`)

```rust
use telemetry_report::{
    ReportBuffer, continue_trace_from_traceparent, current_traceparent, report_layer,
};
use tracing_subscriber::layer::SubscriberExt;

// Register once at startup so MoQT requests attach the active span's traceparent:
moqt::set_trace_context_provider(current_traceparent);

let buffer = ReportBuffer::default();
let subscriber = tracing_subscriber::registry().with(report_layer(buffer.clone()));

tracing::subscriber::with_default(subscriber, || {
    let span = tracing::info_span!(parent: None, "moqt.request");
    // Optional: continue an incoming TRACE_CONTEXT before entering
    continue_trace_from_traceparent(
        &span,
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
    );
    span.in_scope(|| {
        tracing::info!("request started");
    });
});

let (logs, spans) = buffer.take();
```

### Browser / `wasm32` clients (`SpanRecordLayer` + `log_record`)

`report_layer`, `now_ms`, and `random_u64` use `std::time::SystemTime` / `RandomState` and are for **native clients only**. On `wasm32`, construct `SpanRecordLayer { now_ms, random_byte, emit }` with JavaScript clock/RNG callbacks, call `log_record(event, js_now_ms())` in a custom `Layer`, and wrap per-session futures with `scoped(scope_id, fut)` to route spans and logs to the right session.

## Public API

| Item | Role |
| --- | --- |
| `report_layer(buffer)`, `ReportBuffer` | Native `tracing_subscriber::Layer` and bounded ring buffer (`take()` drains `(Vec<ClientLog>, Vec<ClientSpan>)`) |
| `SpanRecordLayer { now_ms, random_byte, emit }` | Target-agnostic span layer emitting closed `ClientSpan`s with their optional `u64` scope |
| `log_record(event, time_ms)` | Converts a `tracing::Event` into a `ClientLog` stamped with the active entered span's trace/span IDs |
| `current_traceparent`, `continue_trace_from_traceparent` | Reads the active W3C `traceparent` (`00-<trace>-<span>-01`) or attaches an incoming one to a new span |
| `scoped(scope, future)`, `in_scope(scope, f)`, `current_scope()` | Associates async tasks and spans with an opaque `u64` scope (e.g. a browser session ID) |

## Behavior & caveats

- **Bounded buffer**: `ReportBuffer` retains at most `telemetry::client_report::MAX_RECORDS` (`100`) logs and `100` spans independently; pushing when full drops the oldest entry.
- **`continue_trace_from_traceparent` requirements**: Must be called **before** the span is first entered, and requires the active subscriber to be `tracing_subscriber::Registry` (otherwise it is a no-op). Strictly validates W3C version `00` lowercase-hex non-zero IDs; invalid values are ignored.
- **Scope inheritance**: New spans inherit their parent span's scope, or fall back to `current_scope()` (`scoped`/`in_scope`). The `u64` scope is routing metadata only and is never serialized into `ClientLog` or `ClientSpan`.

## Further reading

- Architecture: [docs/architecture/telemetry-report/architecture.md](../../docs/architecture/telemetry-report/architecture.md)
- Wire record definitions: [`crates/telemetry`](../telemetry/README.md)
