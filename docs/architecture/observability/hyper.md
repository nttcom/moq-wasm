# `hyper` for the Snapshot and Series API (`observability`)

**Status:** Accepted (2026-10-06)

## What

Add `hyper` and `http-body-util` as direct dependencies of `observability` (using `http-server`'s accept loop). `src/api_server.rs` serves:

- `GET /api/snapshots` (latest snapshot per relay, or historical snapshots via `?at=` and `&previous=`)
- `GET /api/series` (one metric family over a time range)
- `GET /api/namespaces`
- `GET /api/usage`
- `POST /api/client-telemetry`

## Context

The topology UI polls once per second for live state and queries history when a node or link is selected. The API started as two read-only JSON query endpoints and remains a small set of flat routes with no middleware tree.

## Alternatives

- **`axum`:** Shorter route handlers, but adds `axum` and `tower` for a small flat endpoint set.
- **Query ClickHouse's HTTP interface directly from the browser:** Exposes SQL and ClickHouse credentials to clients and cannot serve the live view from in-memory snapshots.

## Decision

Use `hyper` following `services/vts` and `http-server`. Both crates are already in the workspace lock at the same versions, adding no new crate to the dependency tree.
