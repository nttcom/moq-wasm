# `tracing-subscriber` for Telemetry and Spans (`python`)

**Status:** Accepted (2026-11-16)

## What

Add `tracing-subscriber` 0.3 (`default-features = false`, features: `registry`, `std`, matching `wasm`) and the workspace crate `telemetry-report` to `sdk/python`.

## Context

`moqt` and `broadcast` emit `tracing` events and spans, but without a subscriber installed, bot `SUBSCRIBE`/`PUBLISH`/connection events were dropped and requests carried no trace context. `moq_wasm.init` is the Python counterpart to the browser SDK's `init({ logLevel, onLog, onSpan })`, and `telemetry_report::SpanRecordLayer` + `log_record` require a `tracing-subscriber` `Registry`.

## Alternatives

- **Call Python directly from a `tracing` layer:** Acquires the GIL on Tokio workers for every event, stalling session tasks.
- **`pyo3-log`:** Bridges `log` (not `tracing` spans), losing trace context and adding a new crate.

## Decision

`set_log_sink` installs a global `Registry` with a layer queuing `log_record`s (and `SpanRecordLayer` when spans are enabled). A Python daemon thread drains the queue with the GIL released and forwards records to `logging` and `init` callbacks. No new crate enters the lock file. Because the subscriber is global, a process that installs its own cannot also install the module's.
