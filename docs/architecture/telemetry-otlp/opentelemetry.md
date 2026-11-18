# OpenTelemetry crates for the shared telemetry setup

## Status

Accepted

## Date

2026-10-27

## What

`telemetry` depends on `opentelemetry`, `opentelemetry_sdk`,
`opentelemetry-otlp`, `opentelemetry-appender-tracing` and
`tracing-opentelemetry` to export `tracing` spans and events as OTLP traces
and logs.

## Context

The relay already exported OTLP with these crates from its own `logging.rs`.
Following one request across processes (the relay calling the VTS, and later
the ingest bridges and other relays) needs every binary to export the same way
and to share one trace context format. Copying the relay's setup into each
binary would leave N copies to change together.

## Alternatives

- Keep the setup in the relay and copy it into each binary that needs it.
  No new crate, but every fix or new option is N edits.
- Export only stdout and let a log agent (Cloud Logging, the OTel Collector's
  filelog receiver) parse it. No SDK in the binaries, but spans, their
  parent-child links and trace context propagation are lost.
- `tracing-opentelemetry` without the OTLP log bridge, sending logs only to
  stdout. Fewer crates, but logs then lose the trace id that links them to
  their span in the backend.

## Decision

Move the relay's setup unchanged into `telemetry` and keep the same crates.
They were already compiled in the workspace for the relay, so no new crate
enters `Cargo.lock`.
