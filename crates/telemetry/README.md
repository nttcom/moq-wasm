# telemetry

Shared JSON wire schemas connecting clients, relays, and `services/observability`.

> **Workspace crate**: Internal schema contract used by [`telemetry-report`](../telemetry-report/README.md), [`services/relay`](../../services/relay/README.md), [`services/observability`](../../docs/architecture/observability/architecture.md), and `examples/rust/moq-cli`. No async runtime or exporter dependencies (only `anyhow`, `serde`, and `serde_json`); compiles on native targets and `wasm32`.

## Modules

| Module | Wire contract | Producers → Consumer |
| --- | --- | --- |
| `telemetry::client_report` | `POST /api/client-telemetry` payload (`ClientReport`, `ClientLog`, `ClientSpan`, `PlayerStats`, `PublisherStats`) | Browser SDK, Python SDK, `moq-cli` → `services/observability` |
| `telemetry::relay_stats` | 1 Hz `relay.stats.snapshot` record (`RelaySnapshot`, `ProcessStats`, `SessionStats`, `TrackStats`, `SubscriptionStats`) | `services/relay` (via `telemetry-otlp::EventLogger`) → `services/observability` |

## Usage

Inside a function returning `anyhow::Result<()>` (given `snapshot_bytes: &[u8]`):

```rust
use telemetry::client_report::{ClientReport, ClientService};
use telemetry::relay_stats::RelaySnapshot;

// Serialize a client telemetry batch
let report = ClientReport {
    token: None,
    service: ClientService::MoqCli,
    client_id: "cli-1".into(),
    page: "cli".into(),
    logs: vec![],
    stats: vec![],
    publisher_stats: vec![],
    spans: vec![],
};
let _json = serde_json::to_vec(&report)?;

// Decode a relay snapshot stored in OTLP logs
let _snapshot = RelaySnapshot::from_json(snapshot_bytes)?;
```

## Schema conventions & compatibility

- **Batch cap**: `client_report::MAX_RECORDS` (`100`) applies independently per array (`logs`, `stats`, `publisher_stats`, `spans`).
- **Event identifiers**: `relay_stats` exports `NAMESPACE_ROOT` (`"observability"`), `EVENT_NAME` (`"relay.stats.snapshot"`), and `EVENT_ATTRIBUTE` (`"snapshot"`).
- **Units**: Timestamps are Unix milliseconds, durations in `RelaySnapshot` are microseconds (`*_us`), and sizes are bytes. Counters are cumulative (a decrease indicates a restart); `*_since_last_snapshot_*` and `lag_*` are gauges.
- **Backward compatibility**: Newer `RelaySnapshot` fields use `#[serde(default)]`, legacy `SessionPeer` variants stay deserializable for stored history, and `PlayerStatValues::received_kbps` tolerates older scalar values from `moq-wasm <= 0.7.0`.

## Further reading

- Architecture: [docs/architecture/telemetry/architecture.md](../../docs/architecture/telemetry/architecture.md)
- Client `tracing` collector: [`crates/telemetry-report`](../telemetry-report/README.md)
- Server OTLP bootstrap: [`crates/telemetry-otlp`](../telemetry-otlp/README.md)
