# OpenTelemetry Crates for Shared OTLP Export (`telemetry-otlp`)

**Status:** Accepted (2026-10-27)

## What

Depend on `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `opentelemetry-appender-tracing`, and `tracing-opentelemetry` in `telemetry-otlp` to export `tracing` spans and events as OTLP traces and logs.

## Context

`relay` already exported OTLP using these crates in its own `logging.rs`. Tracing requests across services (`relay` → VTS, ingest bridges, cascaded relays) requires every service to export identically and share one trace-context format without duplicating setup code across N binaries.

## Alternatives

- **Copy `relay`'s `logging.rs` into each binary:** Avoids a shared crate, but turns every fix or option into N synchronized edits.
- **Stdout logs parsed by an external log agent (Cloud Logging / OTel Collector `filelog`):** Removes SDK dependencies, but loses spans, parent-child links, and trace-context propagation.
- **`tracing-opentelemetry` without the OTLP log bridge (logs to stdout only):** Fewer crates, but disconnects logs from their span's trace ID in the backend.

## Decision

Extract `relay`'s setup unchanged into `telemetry-otlp` using the exact same crates, adding no new package to `Cargo.lock`.
