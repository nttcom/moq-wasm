# hyper for the snapshot and series API

## Status

Accepted

## Date

2026-10-06

## What

Add `hyper` (`http1`, `server`), `hyper-util` (`tokio`) and `http-body-util`
as direct dependencies of the `observability` crate. `src/api_server.rs` serves
`GET /api/snapshots` (newest snapshot per relay, or the one at `?at=`) and
`GET /api/series` (one metric family over a time range) to the topology page.

## Context

The page polls the server once per second for the live view and asks for
history when a node or link is selected. Two read-only JSON endpoints with
query strings are all it needs; there is no routing tree, middleware or
request body.

## Alternatives

- `axum`. Routing and extractors would shorten the handlers, but it is a new
  crate (plus `tower`) for two endpoints.
- Serving the page's queries straight from ClickHouse's HTTP interface. Saves
  the server code, but exposes SQL and ClickHouse credentials to the browser
  and cannot answer the live view from memory.

## Decision

Use `hyper` the way `crates/vts` already does: the three crates are in the
workspace lock at the same versions, so no new crate enters the dependency
tree, and the server follows the existing accept loop and JSON response
helpers.
