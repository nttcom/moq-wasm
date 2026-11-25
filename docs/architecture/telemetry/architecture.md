# `telemetry` Architecture

## Status
Living document. Update alongside any change to design intent, module boundaries, runtime flow, or invariants.

## Scope
Shared schema types exchanged between telemetry producers and `services/observability`. Depends only on `serde`, `serde_json`, and `anyhow`, so it builds on `wasm32`. Transport stays with producers (`telemetry-otlp` for workspace binaries, `telemetry-report` for clients).

## Client Reports (`telemetry::client_report`)
Defines the JSON payload posted to `observability`'s `/api/client-telemetry`:
- **Types**: `ClientReport` containing `ClientLog`, `ClientSpan`, `PlayerStats`, `PublisherStats`, capped at `MAX_RECORDS` per category.
- **Producers**: `telemetry-report` builds `ClientLog`/`ClientSpan` from `tracing` (forwarded as JSON by wasm/Python, posted directly by `moq-cli`). The browser SDK builds the same JSON in TypeScript and adds `PlayerStats`.
- **Consumer**: `services/observability` parses `ClientReport`, enforces collector/metric limits, and forwards as OTLP.
- **Compatibility**: New fields must be optional so older clients remain readable. Absent trace/span IDs are omitted rather than serialized as `null`.

## Relay Snapshots (`telemetry::relay_stats`)
Defines the 1 Hz snapshot emitted by each relay as an OTLP log record:
- **Wire shape**: `EventName` and body = `EVENT_NAME` (`relay.stats.snapshot`); `EVENT_ATTRIBUTE` (`snapshot`) = `RelaySnapshot::to_json`.
- **Producer**: `services/relay` (`modules/observability`) emits `RelaySnapshot` via `telemetry-otlp`'s `EventLogger`.
- **Consumer**: `services/observability` queries `otel.otel_logs` by `EVENT_NAME`/`EVENT_ATTRIBUTE` and decodes via `RelaySnapshot::from_json`.

See [`observability/architecture.md`](../observability/architecture.md) for field definitions and per-direction visibility.

### Invariants
- **Counters vs. gauges**: Counters are cumulative since session/cache/subscription start; consumers derive rates from deltas and treat a decrease as a restart. Fields named `…_since_last_snapshot_…`, `lag_…`, `ProcessStats::client_sessions`, and `ProcessStats::draining_since_ms` are gauges.
- **Schema evolution**: Every new snapshot field carries `#[serde(default)]` so snapshots from older relays still decode.
- **Units**: Durations in microseconds, sizes in bytes, timestamps in Unix milliseconds.

## Dependencies
- `serde` / `serde_json`: [serde.md](serde.md)
