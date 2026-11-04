# `client-telemetry` Architecture

## Status
Living document. Update this file in the same change whenever the design
intent, module boundaries, runtime flow, or invariants described here change.

## Scope
Turns `tracing` events and closed spans into the JSON log and span records
that the observability server's `/api/client-telemetry` takes, for the clients
that cannot use `telemetry`'s OpenTelemetry SDK: the wasm module (which hands
the records to the page) and `moq-cli` (which posts them itself). Where the
records go stays with each client.

| Item | Role |
| --- | --- |
| `scope` | A number a client gives the spans and events of one of its parts (the wasm module: one session), so it can send each part's records apart: `scoped` polls a future in a scope, `in_scope` runs a closure in one, and `current_scope` reads it |
| `log_record` | One event as `{ timeMs, level, target, message, fields, traceId?, spanId? }`; the ids are those of the innermost span `SpanRecordLayer` saw entered |
| `SpanRecordLayer` | A `Layer` that gives each span a trace and span id, and hands it to `emit` on close as `{ traceId, spanId, parentSpanId?, name, startTimeMs, endTimeMs, fields }`, with its scope |
| `current_traceparent` | The innermost span `SpanRecordLayer` saw entered on this thread, as a W3C `traceparent`; clients give it to `moqt::set_trace_context_provider` |
| `ReportBuffer` | The newest 100 logs and 100 spans since a client last took them, the most one report carries |
| `report_layer` | `log_record` and `SpanRecordLayer` writing into a `ReportBuffer`, timed by `SystemTime` and std's randomly keyed hasher, for the native clients (`moq-cli`, the Python module) |

## Invariants
- A child span takes its parent's trace id; a span without a parent starts a
  new trace.
- A span takes its parent's scope, else the current scope where it was opened.
  The current scope is that of the innermost span `SpanRecordLayer` saw
  entered, else that of the running `scoped` future or `in_scope` call, so an
  event and a span opened inside a span belong to the span's scope even in a
  task that runs in no scope of its own. A span or event in neither has no
  scope.
- A scope is a bare number. What it stands for (a session's token) stays
  with the client and never enters a record.
- `SpanRecordLayer` takes its clock and random source from the client,
  because neither `SystemTime` nor an OS RNG exists on wasm32; only
  `report_layer`, which the wasm module does not use, reads std's. Ids need
  to be unique, not unpredictable.
- Times are milliseconds since the Unix epoch as integers, as the server
  parses them.

## Dependencies
`serde_json`, `tracing` and `tracing-subscriber` (registry only), which the
wasm module already used; see `docs/architecture/wasm/tracing-subscriber.md`.
