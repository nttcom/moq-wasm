# `relay-stats` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`relay-stats` defines the snapshot a relay publishes once per second on
`observability/<relay id>` / `network_stats` (`track_namespace`, `TRACK_NAME`),
so the producer and its consumers share one type:

- `crates/relay` (`modules/observability`) builds and publishes `RelaySnapshot`.
- `crates/observability` subscribes to it on every relay and stores it.

The payload is `RelaySnapshot::to_json`, one JSON object per MoQT object.
`docs/architecture/observability/architecture.md` describes the fields and
what each direction of a session can observe.

## Invariants

- Counters are cumulative since the session, track cache or subscription
  started; consumers derive rates from consecutive snapshots and treat a
  decrease as a restart. Fields named `…_since_last_snapshot_…` and `lag_…`
  are gauges.
- Durations are microseconds, sizes bytes, timestamps Unix milliseconds.
