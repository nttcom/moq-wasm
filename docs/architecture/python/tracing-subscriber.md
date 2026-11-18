# ADR: tracing-subscriber in the python crate

## Status
Accepted

## Date
2026-11-16

## What
`crates/python` depends on `tracing-subscriber` (0.3, already in the workspace) with
`default-features = false, features = ["registry", "std"]`, as `wasm` does, and on the workspace's
`telemetry-report`.

## Context
`moqt` and `broadcast` log and open spans through `tracing`, but the module installed no subscriber,
so a bot's SUBSCRIBE, PUBLISH and connection events were lost and its requests carried no trace
context. The browser SDK's `init({ logLevel, onLog, onSpan })` installs one; `moq_wasm.init` is its
Python counterpart. The records the browser and `moq-cli` build come from
`telemetry_report::SpanRecordLayer` and `log_record`, which need a `tracing-subscriber` `Registry`.

## Alternatives
- **Call into Python from a `tracing` layer** (forward each event to `logging` at once): every event
  would take the GIL on a tokio worker, stalling the session's tasks while Python runs.
- **`pyo3-log`**: bridges the `log` crate to `logging`, not `tracing` spans, so the trace would be
  lost; it is also a new crate.

## Decision
`set_log_sink` installs `registry()` with a layer that queues `log_record`s and, with spans,
`SpanRecordLayer`, as the global default subscriber. A daemon thread takes the queued records with
the GIL released and hands them to `logging` and `init`'s callbacks. The lock file gains no package.
The subscriber is global, so a process that sets its own cannot also install the module's.
