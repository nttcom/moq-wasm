# ADR: tracing-subscriber in the python crate

## Status
Accepted

## Date
2026-11-04

## Context
`moqt` logs and opens spans through `tracing`, but the Python module installed no subscriber, so a bot's
SUBSCRIBE, PUBLISH and connection events never reached HyperDX, and its requests carried no trace context for
the relay to continue. The browser and `moq-cli` already send these records through
`client_telemetry::SpanRecordLayer` and `log_record`, which are `tracing-subscriber` layers.

## Decision
Add **`tracing-subscriber`** (0.3, already in the workspace) to `python` with `default-features = false,
features = ["registry", "std"]`, as `wasm` does. `moqt.start_telemetry()` installs `registry()` with
`client_telemetry::report_layer` as the global default subscriber.

## Consequences

### Positive
- The bots report the same records as the browser and `moq-cli`, built by the same code.
- The crate is already compiled for other workspace members, so the lock file gains no package.

### Negative
- The subscriber is global, so a process that sets its own one cannot also start the module's telemetry.

## Alternatives Considered

### Forward each event to a Python callback (`logging` or loguru)
Every log line would take the GIL on a tokio worker, and the records would lose their trace and span ids
unless Python rebuilt them.

### `pyo3-log`
Bridges the `log` crate to Python's `logging`, not `tracing` spans, so the trace and its context would be lost.
It is also a new crate.
