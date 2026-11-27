# `telemetry-report` Architecture

## Status
Living document. Update alongside any change to design intent, module boundaries, runtime flow, or invariants.

## Scope
Converts `tracing` events and closed spans into `telemetry::client_report` (`ClientLog` / `ClientSpan`) records for `/api/client-telemetry`. Used by clients that cannot run `telemetry-otlp`'s OpenTelemetry SDK: wasm (hands records to JS), Python, the Rust SDK `moq-wasm`, and `moq-cli` (the last two post directly). Record delivery stays with each client.

## API

| Item | Role |
| --- | --- |
| `scope` | Tags spans/events with a numeric scope ID (e.g., per session in wasm) via `scoped` (future), `in_scope` (closure), and `current_scope`. |
| `SpanRecordLayer` | `tracing_subscriber::Layer` that assigns trace/span IDs and emits each closed span as a scoped `ClientSpan`. |
| `log_record` | Builds a `ClientLog` from an event using the innermost entered span's trace/span IDs. |
| `current_traceparent` | Formats the thread's innermost entered span as a W3C `traceparent` (passed to `moqt::set_trace_context_provider`). |
| `continue_trace_from_traceparent` | Attaches a not-yet-entered span to a remote parent `traceparent` (used in wasm for incoming requests carrying `TRACE_CONTEXT`). |
| `ReportBuffer` | Ring-like buffer keeping the newest `MAX_RECORDS` logs and spans between drains. |
| `report_layer` | Ready-made layer (`log_record` + `SpanRecordLayer` → `ReportBuffer`) using `SystemTime` (`now_ms`) and std's keyed hasher (`random_u64`). Used by `moq-cli`; Python and the Rust SDK assemble the same parts directly to queue records per session token. |

## Invariants

- **Trace inheritance**: Child spans inherit their parent's trace ID. Root spans start a new trace unless `continue_trace_from_traceparent` attaches a remote parent before entry (no-op without a `Registry` subscriber).
- **Scope inheritance**: Spans inherit their parent span's scope, falling back to the active `scoped`/`in_scope` call. Active scope resolves innermost entered span first, then `scoped`/`in_scope`, so nested tasks inherit the span's scope; outside both, scope is absent.
- **Opaque scope IDs**: Scopes are bare integers; their meaning (e.g., session token) stays client-side and never enters records.
- **Injectable clock/RNG**: `SpanRecordLayer` takes clock and random closures because `wasm32` lacks `SystemTime` and an OS RNG (only `report_layer` reads `std`). IDs need uniqueness, not cryptographic unpredictability.
- **Wire contract**: Timestamps are integer Unix milliseconds. JSON matches `telemetry::client_report` so client and server cannot drift.

## Dependencies
`telemetry`, `serde_json`, `tracing`, `tracing-subscriber` (registry only; see [`wasm/tracing-subscriber.md`](../wasm/tracing-subscriber.md)).
