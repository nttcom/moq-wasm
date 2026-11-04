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
| `log_record` | One event as `{ timeMs, level, target, message, fields, traceId?, spanId? }`; the ids are those of the innermost span `SpanRecordLayer` saw entered |
| `SpanRecordLayer` | A `Layer` that gives each span a trace and span id, and hands it to `emit` on close as `{ traceId, spanId, parentSpanId?, name, startTimeMs, endTimeMs, fields }` |
| `current_traceparent` | The innermost span `SpanRecordLayer` saw entered on this thread, as a W3C `traceparent`; clients give it to `moqt::set_trace_context_provider` |

## Invariants
- A child span takes its parent's trace id; a span without a parent starts a
  new trace.
- The clock and the random source are injected, because neither
  `SystemTime` nor an OS RNG exists on wasm32. Ids need to be unique, not
  unpredictable.
- Times are milliseconds since the Unix epoch as integers, as the server
  parses them.

## Dependencies
`serde_json`, `tracing` and `tracing-subscriber` (registry only), which the
wasm module already used; see `docs/architecture/wasm/tracing-subscriber.md`.
