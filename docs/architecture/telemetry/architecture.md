# `telemetry` Architecture

## Status
Living document. Update this file in the same change whenever the design
intent, module boundaries, runtime flow, or invariants described here change.

## Scope
One `init` that installs the global `tracing` subscriber for a binary of the
workspace, so every binary writes the same readable log and exports the same
OTLP traces and logs. What to put in a span stays with each crate.
The `relay_stats` module defines the snapshot format shared by the relay
and the observability server.

## Outputs

| Output | Filter variable | Content |
| --- | --- | --- |
| stdout, or stderr with `LogOutput::Stderr` | `<SERVICE>_STDOUT_FILTER` | `tracing_subscriber::fmt` text with line numbers |
| OTLP traces | `<SERVICE>_OTEL_FILTER` | spans only, through `tracing-opentelemetry` |
| OTLP logs | `<SERVICE>_LOG_FILTER` | events, through `opentelemetry-appender-tracing` |

`<SERVICE>` is `TelemetryConfig::service_name` in upper case with `-` as `_`
(`relay` → `RELAY_STDOUT_FILTER`). Each filter falls back to `RUST_LOG`, and
each `default_directives` entry, and `telemetry=info` for the startup line,
is added unless the filter already names its target, so
`RUST_LOG=quinn=debug` still logs `relay=info`.

The OTLP exporters run only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set and
`OTEL_SDK_DISABLED` is not. This departs from the OpenTelemetry default, which
exports to `localhost:4317` when no endpoint is given: a binary deployed
without a collector would keep sending to nothing. The other standard
variables apply as usual: `OTEL_EXPORTER_OTLP_PROTOCOL` picks `grpc`
(default) or `http/protobuf`, `OTEL_EXPORTER_OTLP_HEADERS` is read by
`opentelemetry-otlp`, and `OTEL_SERVICE_NAME` overrides `service.name`. The resource carries
`service.name`, `service.version` and the caller's `resource_attributes`.

A log emitted inside an exported span carries that span's trace and span id:
`tracing-opentelemetry` activates the span's OpenTelemetry context on enter,
and the log bridge reads the active context.

When OTLP is on, `init` also installs the W3C Trace Context propagator as the
global propagator.

## Event records

`TelemetryGuard::event_logger` returns an `EventLogger` (`None` while OTLP is
off) for records that carry data rather than a message: they go to the OTLP
log exporter only, past the `tracing` filters and stdout, so a payload sent
every second neither floods the readable log nor depends on a filter. Each
record has an `EventName`, that name as its body, INFO severity and the data
as one string attribute. The data is not the body because the ClickStack
collector parses any body containing a JSON object into one attribute per
field, which would store it twice.

## Metrics

`TelemetryGuard::meter` returns a `Meter` (`None` while OTLP is off) whose
instruments the OTLP metric exporter sends every
`OTEL_METRIC_EXPORT_INTERVAL` ms, 10 s by default rather than the
OpenTelemetry 60 s, so a HyperDX chart of the last hour has enough points.
The relay records the relay-wide values of its stats snapshot through
`relay_stats::ProcessGauges` (see the observability architecture).

## Trace context

A span's context crosses a process boundary as a W3C `traceparent`.
`inject_trace_context` writes it into an `http::HeaderMap`, and
`continue_trace_from` makes a server's span a child of the context a request
carries. The relay injects it into its `POST /verify` call and the VTS
continues it, so one trace covers both processes.

`current_traceparent` returns the current span's `traceparent` as a string
and `continue_trace_from_traceparent` continues one, for carriers other than
HTTP headers: the relay gives `current_traceparent` to `moqt` as its trace
context provider, so a request it forwards upstream carries it in
TRACE_CONTEXT, and continues the TRACE_CONTEXT a client's request carries.
All four are no-ops while OTLP is off, because the global propagator is then
a no-op.

## Lifecycle

`init` fails when a subscriber is already installed or the OTLP protocol is
unknown. It returns a `TelemetryGuard` whose drop shuts the providers down,
flushing the batches still queued, so a binary holds it for the life of
`main`.

## Relay snapshots

`telemetry::relay_stats` defines the snapshot a relay publishes once per
second on `observability/<relay id>` / `network_stats` (`track_namespace`,
`TRACK_NAME`), so the producer and its consumers share one type:

- `services/relay` (`modules/observability`) builds and publishes `RelaySnapshot`.
- `services/observability` subscribes to it on every relay and stores it.

The payload is `RelaySnapshot::to_json`, one JSON object per MoQT object.
The [observability architecture](../observability/architecture.md) describes
the fields and what each direction of a session can observe. The snapshot
types do not require `init` or an OTLP exporter to be used.

### Invariants

- Counters are cumulative since the session, track cache or subscription
  started; consumers derive rates from consecutive snapshots and treat a
  decrease as a restart. Fields named `…_since_last_snapshot_…` and `lag_…`
  are gauges, as are `ProcessStats::client_sessions` and
  `ProcessStats::draining_since_ms`.
- A field added to the snapshot has `#[serde(default)]`, so a consumer reads
  the snapshots of relays that predate it.
- Durations are microseconds, sizes bytes, timestamps Unix milliseconds.

## Dependencies

`opentelemetry.md` records why the OpenTelemetry crates are used, and
`http.md` why the trace context helpers take an `http::HeaderMap`, and
`serde.md` why the relay snapshots use Serde and JSON.
