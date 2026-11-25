# `futures` for Polling Subgroup Readers in One Task (`moqt`)

**Status:** Accepted (2026-10-05)

## What

Add `futures` as a dependency of `moqt`. `TrackReader` stores per-subgroup reader futures in `futures::stream::FuturesUnordered` and polls them from its single accept task.

## Context

`TrackReader` previously used `tokio::task::JoinSet`, which requires a Tokio runtime and cannot run in browsers on `wasm-bindgen-futures` (see the executor seam in [architecture.md](architecture.md)). `TrackReader` needs concurrent stream polling without a multi-task runtime.

## Alternatives

- **Spawn each reader via `modules/executor` and track handles:** Requires a join-capable `JoinHandle` and an abort-on-drop container to match `JoinSet`'s cancellation—extra executor surface for a single call site.
- **Manual `std::task` polling:** Reimplements `FuturesUnordered`.
- **`JoinSet` on native + `cfg` fallback on `wasm32`:** Maintains two code paths for identical behavior.

## Decision

Use `FuturesUnordered` driven by `tokio::select!` alongside the stream accept loop. `futures` is already in the workspace lock (`services/ingest-gateway`), adding no new crate to the dependency tree.

`TrackReader` is client-side only (`broadcast`, `recorder`, `live-translate`, `moq-cli`); the relay uses its own data path, so collapsing subgroup readers into one task does not affect relay throughput.
