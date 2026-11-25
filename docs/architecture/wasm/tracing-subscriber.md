# `tracing-subscriber` for Structured Browser Telemetry (`wasm`)

**Status:** Accepted (2026-10-31)

## What

Add `tracing-subscriber` 0.3 (`default-features = false`, features: `registry`, `std`) as a `wasm32`-only dependency of `wasm`.

- `setLogSink` installs a global `registry()` with a `LevelFilter` and a custom `Layer` that converts events via `telemetry_report::log_record` into `{ timeMs, level, target, message, fields }` JSON records for the page's `onLog` callback.
- When `init({ onSpan })` is set, a second layer (`telemetry_report::SpanRecordLayer`, `INFO` and above) emits each closed span as a `SpanRecord`.
- Record construction lives in `telemetry-report` (target-independent) so it is unit-tested on the host.

## Context

`moqt`, `broadcast`, and `wasm` emit `tracing` events, but without a subscriber on `wasm32` every event was discarded—including `tracing::warn!` dropped-frame diagnostics in `sdk/js/src/media_broadcast.rs` and client telemetry sent to ClickStack. Pages need structured records via `init({ onLog, logLevel })` rather than pre-formatted console strings.

## Alternatives

- **`tracing-wasm` / `tracing-web`:** Format directly to `console.*` as strings, losing structured fields for forwarding and adding a new crate.
- **Custom `tracing::Subscriber`:** Avoids `Registry`, but requires hand-writing span ID allocation and enter/exit bookkeeping that `Registry` already provides.

## Decision

Use `tracing-subscriber`'s `Registry` with custom layers on `wasm32`.

## Consequences

- **Positive:** Typed fields, target, and level without text parsing; `LevelFilter` drops below-threshold events before allocation; already in `Cargo.lock` (zero new transitive crates).
- **Negative:** `Registry` pulls `sharded-slab` and `thread_local` into the wasm binary.
- **Neutral:** `fmt` stays off because its default timer reads `SystemTime` (which panics on `wasm32-unknown-unknown`). The subscriber is global, so a second `init({ onLog })` rejects.

## References

- [tracing-subscriber `Layer`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html)
- [browser-sdk.md](browser-sdk.md) (core API)
