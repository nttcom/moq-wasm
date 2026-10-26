# `telemetry` Architecture

## Status
Living document. Update this file in the same change whenever the design
intent, module boundaries, runtime flow, or invariants described here change.

## Scope
One `init` that installs the global `tracing` subscriber for a binary of the
workspace, so every binary writes the same stdout format and exports the same
OTLP traces and logs. The relay is its first user; what to put in a span stays
with each crate.

## Outputs

| Output | Filter variable | Content |
| --- | --- | --- |
| stdout | `<SERVICE>_STDOUT_FILTER` | `tracing_subscriber::fmt` text with line numbers |
| OTLP traces | `<SERVICE>_OTEL_FILTER` | spans only, through `tracing-opentelemetry` |
| OTLP logs | `<SERVICE>_LOG_FILTER` | events, through `opentelemetry-appender-tracing` |

`<SERVICE>` is `TelemetryConfig::service_name` in upper case with `-` as `_`
(`relay` → `RELAY_STDOUT_FILTER`). Each filter falls back to `RUST_LOG`, and
each `default_directives` entry, and `telemetry=info` for the startup line,
is added unless the filter already names its target, so
`RUST_LOG=quinn=debug` still logs `relay=info`.

The OTLP exporters follow the standard OpenTelemetry variables:
`OTEL_SDK_DISABLED` turns both off, `OTEL_EXPORTER_OTLP_PROTOCOL` picks `grpc`
(default) or `http/protobuf`, `OTEL_EXPORTER_OTLP_ENDPOINT` /
`OTEL_EXPORTER_OTLP_HEADERS` are read by `opentelemetry-otlp`, and
`OTEL_SERVICE_NAME` overrides `service.name`. The resource carries
`service.name`, `service.version` and the caller's `resource_attributes`.

A log emitted inside an exported span carries that span's trace and span id:
`tracing-opentelemetry` activates the span's OpenTelemetry context on enter,
and the log bridge reads the active context.

When OTLP is on, `init` also installs the W3C Trace Context propagator as the
global propagator.

## Lifecycle

`init` fails when a subscriber is already installed or the OTLP protocol is
unknown. It returns a `TelemetryGuard` whose drop shuts the providers down,
flushing the batches still queued, so a binary holds it for the life of
`main`.

## Dependencies

`opentelemetry.md` records why the OpenTelemetry crates are used.
