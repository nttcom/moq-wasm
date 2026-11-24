# telemetry-otlp

Shared `tracing` and OpenTelemetry (OTLP) bootstrap for workspace server binaries: stdout/stderr logs, optional OTLP traces/logs/metrics, `EventLogger` for structured data records, and W3C Trace Context propagation.

> **Workspace crate**: Internal library used by every service under `services/` (`relay`, `ingest-gateway`, `egress-gateway`, `sip-gateway`, `recorder`, `vts`, `console`, `bot-dispatcher`, `live-translate`, `observability`, `temporary-apps`) and `auth-token` (`vts-client`). Native-only.

## Usage

Call `init()` once at startup and hold the returned `TelemetryGuard` until process exit so `Drop` flushes buffered OTLP exports (inside a function returning `anyhow::Result<()>`):

```rust
use std::time::SystemTime;
use telemetry_otlp::{
    LogOutput, TelemetryConfig, continue_trace_from, current_trace_headers, init,
};

let guard = init(TelemetryConfig {
    service_name: "relay",
    service_version: env!("CARGO_PKG_VERSION"),
    default_directives: &["relay=info", "moqt=info"],
    resource_attributes: vec![("host.name", "relay-1".into())],
    log_output: LogOutput::Stdout,
})?;

// Emit a structured OTLP event bypassing tracing filters and stdout
if let Some(events) = guard.event_logger("relay.stats") {
    events.emit(
        telemetry::relay_stats::EVENT_NAME,
        SystemTime::now(),
        telemetry::relay_stats::EVENT_ATTRIBUTE,
        snapshot_json, // String
    );
}

// Outgoing HTTP trace headers
let _headers = current_trace_headers();

// Incoming HTTP trace continuation (call BEFORE entering the span)
let req_span = tracing::info_span!("handle_request");
continue_trace_from(&req_span, &incoming_headers);
let _entered = req_span.entered();
```

## Public API

| Item | Role |
| --- | --- |
| `init(TelemetryConfig)` → `TelemetryGuard` | Installs the global `tracing` subscriber; `guard.event_logger(scope)` and `guard.meter(scope)` expose OTLP handles |
| `EventLogger::emit` | Writes a structured record directly to the OTLP logger provider (bypasses `tracing` filters and stdout) |
| `inject_trace_context`, `current_trace_headers`, `continue_trace_from` | W3C Trace Context propagation over HTTP headers |
| `current_traceparent`, `continue_trace_from_traceparent` | Raw W3C `traceparent` propagation (used for MoQT `TRACE_CONTEXT` parameters) |
| `non_empty_env(name)` | Returns `None` if `name` is unset or empty (`""`); whitespace-only values return `Some` |

Optional Cargo feature: `testing` exposes `testing::set_default` to avoid `tracing-core` single-dispatcher interest caching in parallel tests.

## Environment variables

### OTLP exporter

| Variable | Effect |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Required non-empty to enable OTLP export (unset **disables** OTLP instead of defaulting to `localhost:4317`) |
| `OTEL_SDK_DISABLED` | `1` / `true` / `yes` / `on` disables OTLP even when an endpoint is set |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` (default) or `http/protobuf` / `http/proto` / `http`; other values fail `init()` |
| `OTEL_EXPORTER_OTLP_HEADERS` | Extra OTLP exporter headers (read by `opentelemetry-otlp`) |

### Service metadata, metrics & log filters

| Variable | Effect |
| --- | --- |
| `OTEL_SERVICE_NAME` | Overrides `service.name` resource attribute (does not change filter env var prefix) |
| `OTEL_METRIC_EXPORT_INTERVAL` | Metric export interval in ms (defaults to `10000` ms instead of OTel's 60 s) |
| `<SERVICE>_STDOUT_FILTER`, `<SERVICE>_OTEL_FILTER`, `<SERVICE>_LOG_FILTER` | Per-layer `EnvFilter` (`<SERVICE>` is uppercased `service_name` with `-` → `_`); falls back to `RUST_LOG` + `default_directives` |

## Behavior & caveats

- **No-op when OTLP is off**: Without `OTEL_EXPORTER_OTLP_ENDPOINT` (or with `OTEL_SDK_DISABLED=true`), `event_logger()`, `meter()`, and `current_traceparent()` return `None`, and trace helpers are no-ops.
- **Span timing**: Call `continue_trace_from` / `continue_trace_from_traceparent` on a `Span` **before** entering it.
- **Cloud Run `traceparent` preservation**: `inject_trace_context` writes both `traceparent` and `x-original-traceparent` because Cloud Run rewrites `traceparent`; `continue_trace_from` reads `x-original-traceparent` first.
- **Single initialization**: `init()` returns an error if a global `tracing` subscriber is already installed.

## Further reading

- Architecture: [docs/architecture/telemetry-otlp/architecture.md](../../docs/architecture/telemetry-otlp/architecture.md)
