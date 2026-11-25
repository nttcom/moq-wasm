# `hyper` for the Recording API (`recorder`)

**Status:** Accepted (2026-10-16)

## What

Add `hyper` (`http1`, `server`) and `http-body-util` to `services/recorder` (using `http-server`'s accept loop). `src/api_server.rs` serves `recorder serve`'s API for listing room recordings, signing file URLs, and publishing replays.

## Context

Browser pages need to list and replay recordings over a handful of JSON endpoints with small request bodies and CORS headers. `observability` already serves its API on these crates (see [observability/hyper.md](../observability/hyper.md)), so they are already in the workspace lock.

## Alternatives

- **`axum`:** Shorter handlers, but adds `axum` and `tower` for four routes already handled by a single `match` in `handle`.
- **Spawn `recorder` CLI subprocesses from a separate service:** Duplicates room tracking and forces log parsing to report progress.

## Decision

Use `hyper` directly, matching `observability`. JSON is handled with `serde_json` (already a dependency of `recorder`), avoiding `serde` derive.
