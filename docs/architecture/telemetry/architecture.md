# `telemetry` Architecture

## Status
Living document. Update this file in the same change whenever the design
intent, module boundaries, runtime flow, or invariants described here change.

## Scope
The records the observability server reads, as types its producers write
and it decodes, so both sides share one definition. The crate depends only on
Serde and JSON: how a record is sent (`telemetry-otlp` for the workspace
binaries) stays with the producer.

## Relay snapshots

`telemetry::relay_stats` defines the snapshot a relay sends once per second
as an OTLP log record (`EVENT_NAME` `relay.stats.snapshot` as the record's
`EventName` and body, `RelaySnapshot::to_json` in its `EVENT_ATTRIBUTE`
`snapshot`), so the producer and its consumers share one type:

- `services/relay` (`modules/observability`) builds `RelaySnapshot` and
  emits it through `telemetry-otlp`'s `EventLogger`.
- `services/observability` fills its tables from the records in
  `otel.otel_logs`, selected by the same `EVENT_NAME` and `EVENT_ATTRIBUTE`,
  and decodes a stored snapshot with `RelaySnapshot::from_json`.

The [observability architecture](../observability/architecture.md) describes
the fields and what each direction of a session can observe.

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

`serde.md` records why the records use Serde and JSON.
