# futures for polling subgroup readers inside one task

## Status

Accepted

## Date

2026-10-05

## What

Add `futures` as a dependency of the `moqt` crate. `TrackReader` keeps the
per-subgroup reader futures in a `futures::stream::FuturesUnordered` and polls
them from its single accept task.

## Context

`TrackReader` used to put each subgroup reader into a `tokio::task::JoinSet`.
`JoinSet` spawns onto the tokio runtime, which does not exist in the browser,
where the crate has to run on `wasm-bindgen-futures` (see the executor seam in
`architecture.md`). The reader needs a way to run many stream reads
concurrently that does not depend on a multi-task runtime.

## Alternatives

- Spawn each reader through `modules/executor` and keep the handles. Needs a
  join-capable `JoinHandle` and an abort-on-drop container to keep the prompt
  cancellation `JoinSet` gave; more executor surface for one call site.
- Implement the unordered polling by hand with `std::task`. Reimplements
  `FuturesUnordered`.
- Keep `JoinSet` natively and switch by `cfg` on wasm32. Two code paths for
  the same behaviour.

## Decision

Use `FuturesUnordered` driven by `tokio::select!` next to the stream accept
loop. `futures` is already in the workspace lock (`crates/wasm` and
`crates/live-ingest` depend on it), so no new crate enters the dependency
tree. `TrackReader` is used by `moq-cli` only; the relay has its own data
path, so moving the readers from separate tasks into one task does not affect
relay throughput.
