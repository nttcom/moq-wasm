# ADR: tracing-subscriber in the wasm crate

## Status
Accepted

## Date
2026-10-31

## Context
`moqt`, `broadcast` and `wasm` log through `tracing`, but the browser build installs no subscriber, so every
event inside the wasm module is discarded. Pages need these logs: a `tracing::warn!` in
`crates/wasm/src/media_broadcast.rs` is the only report of a dropped frame, and the browser telemetry plan forwards
client logs to ClickStack.

The SDK should hand each event to the page as a structured record (`{ timeMs, level, target, message, fields }`)
through `init({ onLog, logLevel })`, so the page decides whether to print, buffer or send it.

## Decision
Add **`tracing-subscriber`** (0.3, already in the workspace) to `wasm` with `default-features = false,
features = ["registry", "std"]`.

On wasm32, `setLogSink` installs `registry()` with a `LevelFilter` and one custom `Layer` as the global default
subscriber. The layer turns each event into a JSON record and calls the page's callback. Building the record
(`log_record.rs`) does not depend on wasm32, so it is unit tested on the host.

## Consequences

### Positive
- `Layer` and `Visit` give typed fields, the target and the level without parsing formatted text.
- `LevelFilter` drops events below `logLevel` before any record is built.
- The crate is already compiled for other workspace members, so it adds no new transitive dependency to the
  lock file.

### Negative
- The registry pulls `sharded-slab` and `thread_local` into the wasm binary, which only need the event path.

### Neutral
- The `fmt` feature stays off: its default timer reads `SystemTime`, which panics on `wasm32-unknown-unknown`.
- The subscriber is global, so a page sets one sink; a second `init({ onLog })` rejects.

## Alternatives Considered

### `tracing-wasm` / `tracing-web`
Both write to `console.*` in their own format. The page would get strings, not records, and could not forward
them with fields intact. They also add a crate the workspace does not have.

### Implementing `tracing::Subscriber` directly
Avoids the registry, but means writing span id allocation and the enter/exit bookkeeping by hand. The registry
already does this and the workspace already depends on it.

## References
- [tracing-subscriber `Layer`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html)
- `docs/architecture/wasm/browser-sdk.md` (core API)
